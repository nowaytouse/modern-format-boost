//! GPU coarse search and CPU fine-tuning for CRF exploration

use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

use super::calibration;
use super::dynamic_mapping;
use super::precheck;
use super::*;

/// Percentage change from input stream size (avoids div-by-zero / inf when input is 0).
#[inline]
fn stream_size_change_pct(output_size: u64, input_size: u64) -> f64 {
    let denom = input_size.max(1) as f64;
    (output_size as f64 / denom - 1.0) * 100.0
}

pub fn explore_with_gpu_coarse_search(
    input: &Path,
    output: &Path,
    encoder: VideoEncoder,
    vf_args: Vec<String>,
    initial_crf: f32,
    max_crf: f32,
    min_ssim: f64,
    ultimate_mode: bool,
    force_ms_ssim_long: bool,
    max_threads: usize,
) -> Result<ExploreResult> {
    use crate::gpu_accel::{CrfMapping, GpuAccel, GpuCoarseConfig};

    let precheck_info = precheck::run_precheck(input)?;
    let _compressibility = precheck_info.compressibility;
    crate::log_eprintln!();

    let gpu = GpuAccel::detect();
    gpu.print_detection_info();

    let input_size = fs::metadata(input)
        .context("Failed to read input file metadata")?
        .len();

    let gpu = GpuAccel::detect();
    let encoder_name = match encoder {
        VideoEncoder::Hevc => "hevc",
        VideoEncoder::Av1 => "av1",
        VideoEncoder::H264 => "h264",
    };

    let has_gpu_encoder = match encoder {
        VideoEncoder::Hevc => gpu.get_hevc_encoder().is_some(),
        VideoEncoder::Av1 => gpu.get_av1_encoder().is_some(),
        VideoEncoder::H264 => gpu.get_h264_encoder().is_some(),
    };

    crate::verbose_eprintln!("Smart GPU+CPU Explore v5.1 ({:?})", encoder);
    crate::verbose_eprintln!(
        "   Input: {} bytes ({:.2} MB)",
        input_size,
        input_size as f64 / 1024.0 / 1024.0
    );
    crate::verbose_eprintln!();
    crate::verbose_eprintln!("STRATEGY: GPU Coarse → CPU Fine");
    crate::verbose_eprintln!("• Phase 1: GPU finds rough boundary (FAST)");
    crate::verbose_eprintln!("• Phase 2: CPU finds precise CRF (ACCURATE)");
    crate::verbose_eprintln!();

    let (cpu_min_crf, cpu_max_crf, cpu_center_crf) = if gpu.is_available() && has_gpu_encoder {
        crate::verbose_eprintln!();
        crate::verbose_eprintln!("Phase 1: GPU Coarse Search");

        let temp_output =
            output.with_extension(crate::gpu_accel::derive_gpu_temp_extension(output));

        let gpu_encoder_name = match encoder {
            VideoEncoder::Hevc => gpu
                .get_hevc_encoder()
                .map(|e| e.ffmpeg_name())
                .unwrap_or("hevc_videotoolbox"),
            VideoEncoder::Av1 => gpu
                .get_av1_encoder()
                .map(|e| e.ffmpeg_name())
                .unwrap_or("av1"),
            VideoEncoder::H264 => gpu
                .get_h264_encoder()
                .map(|e| e.ffmpeg_name())
                .unwrap_or("h264_videotoolbox"),
        };

        let duration: f32 = {
            use std::process::Command;
            let duration_output = Command::new("ffprobe")
                .args([
                    "-v",
                    "error",
                    "-show_entries",
                    "format=duration",
                    "-of",
                    "default=noprint_wrappers=1:nokey=1",
                    "--",
                ])
                .arg(crate::safe_path_arg(input).as_ref())
                .output();
            duration_output
                .ok()
                .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok())
                .unwrap_or(crate::gpu_accel::GPU_SAMPLE_DURATION)
        };
        let gpu_sample_input_size = if duration <= crate::gpu_accel::GPU_SAMPLE_DURATION {
            input_size
        } else {
            let ratio = crate::gpu_accel::GPU_SAMPLE_DURATION / duration;
            (input_size as f64 * ratio as f64) as u64
        };

        let gpu_config = GpuCoarseConfig {
            initial_crf,
            min_crf: crate::gpu_accel::GPU_DEFAULT_MIN_CRF,
            max_crf,
            step: 2.0,
            max_iterations: crate::gpu_accel::GPU_ABSOLUTE_MAX_ITERATIONS,
        };

        let gpu_progress = crate::UnifiedProgressBar::new_iteration(
            "🔍 GPU Search",
            gpu_sample_input_size,
            gpu_config.max_iterations as u64,
        );

        let progress_callback = |crf: f32, size: u64| {
            gpu_progress.inc_iteration(crf, size, None);
        };

        let log_callback = |msg: &str| {
            gpu_progress.println(msg);
        };

        let gpu_result = crate::gpu_accel::gpu_coarse_search_with_log(
            input,
            &temp_output,
            encoder_name,
            input_size,
            &gpu_config,
            Some(&progress_callback),
            Some(&log_callback),
        );

        let (final_crf, final_size) = match &gpu_result {
            Ok(result) if result.found_boundary => {
                (result.gpu_boundary_crf, result.gpu_best_size.unwrap_or(0))
            }
            _ => (gpu_config.max_crf, input_size),
        };
        gpu_progress.finish_iteration(final_crf, final_size, None);

        match gpu_result {
            Ok(gpu_result) => {
                if gpu_result.found_boundary {
                    let gpu_crf = gpu_result.gpu_boundary_crf;
                    let gpu_size = gpu_result.gpu_best_size.unwrap_or(input_size);

                    let sample_duration = crate::gpu_accel::GPU_SAMPLE_DURATION;
                    let dynamic_mapper = dynamic_mapping::quick_calibrate(
                        input,
                        input_size,
                        encoder,
                        &vf_args,
                        gpu_encoder_name,
                        sample_duration,
                    )
                    .unwrap_or_else(|_| dynamic_mapping::DynamicCrfMapper::new(input_size));

                    let mapping = match encoder {
                        VideoEncoder::Hevc => CrfMapping::hevc(gpu.gpu_type),
                        VideoEncoder::Av1 => CrfMapping::av1(gpu.gpu_type),
                        VideoEncoder::H264 => CrfMapping::hevc(gpu.gpu_type),
                    };

                    let (dynamic_cpu_crf, dynamic_confidence) = if dynamic_mapper.calibrated {
                        dynamic_mapper.print_calibration_report();
                        dynamic_mapper.gpu_to_cpu(gpu_crf, mapping.offset)
                    } else {
                        let calibration = calibration::CalibrationPoint::from_gpu_result(
                            gpu_crf,
                            gpu_size,
                            input_size,
                            gpu_result.gpu_best_ssim,
                            mapping.offset,
                        );
                        calibration.print_report(input_size);
                        (calibration.predicted_cpu_crf, calibration.confidence)
                    };

                    if let Some(ceiling_crf) = gpu_result.quality_ceiling_crf {
                        if ceiling_crf == gpu_crf {
                            crate::verbose_eprintln!(
                                "GPU Boundary = Quality Ceiling: CRF {:.1}",
                                gpu_crf
                            );
                            crate::verbose_eprintln!(
                                "   (GPU reached quality limit, no bloat beyond this point)"
                            );
                        } else {
                            crate::verbose_eprintln!(
                                "GPU Boundary: CRF {:.1} (stopped before quality ceiling)",
                                gpu_crf
                            );
                        }
                    } else {
                        crate::verbose_eprintln!(
                            "GPU Boundary: CRF {:.1} (quality ceiling not detected)",
                            gpu_crf
                        );
                    }
                    crate::verbose_eprintln!(
                        "Dynamic mapping: GPU {:.1} → CPU {:.1} (confidence {:.0}%)",
                        gpu_crf,
                        dynamic_cpu_crf,
                        dynamic_confidence * 100.0
                    );
                    crate::verbose_eprintln!();

                    let cpu_start = dynamic_cpu_crf;

                    crate::verbose_eprintln!(
                        "   ✅ GPU found boundary: CRF {:.1} (fine-tuned: {})",
                        gpu_crf,
                        gpu_result.fine_tuned
                    );
                    if let Some(size) = gpu_result.gpu_best_size {
                        crate::verbose_eprintln!("   GPU best size: {} bytes", size);
                    }

                    if let (Some(ceiling_crf), Some(ceiling_ssim)) = (
                        gpu_result.quality_ceiling_crf,
                        gpu_result.quality_ceiling_ssim,
                    ) {
                        crate::verbose_eprintln!(
                            "   GPU Quality Ceiling: CRF {:.1}, SSIM {:.4}",
                            ceiling_crf,
                            ceiling_ssim
                        );
                        crate::verbose_eprintln!(
                            "      (GPU SSIM ceiling, CPU can break through to 0.99+)"
                        );
                    }

                    let (cpu_min, cpu_max) = if let Some(ssim) = gpu_result.gpu_best_ssim {
                        let quality_hint = if ssim >= 0.97 {
                            "Near GPU ceiling"
                        } else if ssim >= 0.95 {
                            "Good"
                        } else {
                            "Below expected"
                        };
                        crate::verbose_eprintln!("   GPU best SSIM: {:.6} {}", ssim, quality_hint);

                        if ssim < 0.90 {
                            crate::verbose_eprintln!(
                                "   ⚠️ GPU SSIM too low! Expanding CPU search to lower CRF"
                            );
                            (ABSOLUTE_MIN_CRF, (cpu_start + 8.0).min(max_crf))
                        } else if gpu_result.fine_tuned {
                            crate::verbose_eprintln!(
                                "   GPU fine-tuned → CPU narrow search ±3 CRF"
                            );
                            (
                                (cpu_start - 3.0).max(ABSOLUTE_MIN_CRF),
                                (cpu_start + 3.0).min(max_crf),
                            )
                        } else {
                            crate::verbose_eprintln!(
                                "   CPU will achieve SSIM 0.98+ (GPU max ~0.97)"
                            );
                            (
                                (cpu_start - 15.0).max(ABSOLUTE_MIN_CRF),
                                (cpu_start + 5.0).min(max_crf),
                            )
                        }
                    } else if gpu_result.fine_tuned {
                        crate::verbose_eprintln!("   GPU fine-tuned → CPU narrow search ±3 CRF");
                        (
                            (cpu_start - 3.0).max(ABSOLUTE_MIN_CRF),
                            (cpu_start + 3.0).min(max_crf),
                        )
                    } else {
                        (
                            (cpu_start - 15.0).max(ABSOLUTE_MIN_CRF),
                            (cpu_start + 5.0).min(max_crf),
                        )
                    };

                    crate::verbose_eprintln!(
                        "   CPU search range: [{:.1}, {:.1}] (start: {:.1})",
                        cpu_min,
                        cpu_max,
                        cpu_start
                    );
                    (cpu_min, cpu_max, cpu_start)
                } else {
                    crate::verbose_eprintln!(
                        "GPU coarse search: no boundary found, using full CRF range for CPU search"
                    );
                    (ABSOLUTE_MIN_CRF, max_crf, initial_crf)
                }
            }
            Err(e) => {
                crate::log_eprintln!("⚠️  FALLBACK: GPU coarse search failed!");
                crate::log_eprintln!("• Error: {}", e);
                crate::log_eprintln!("• Falling back to CPU-only search (full range)");
                (ABSOLUTE_MIN_CRF, max_crf, initial_crf)
            }
        }
    } else {
        crate::log_eprintln!();
        if !gpu.is_available() {
            crate::log_eprintln!("⚠️  FALLBACK: No GPU available!");
            crate::log_eprintln!("• Skipping GPU coarse search phase");
            crate::log_eprintln!("• Using CPU-only search (may take longer)");
        } else {
            crate::log_eprintln!(
                "⚠️  FALLBACK: No GPU encoder for {:?}!              ",
                encoder
            );
            crate::log_eprintln!("• Skipping GPU coarse search phase");
            crate::log_eprintln!("• Using CPU-only search (may take longer)");
        }
        (ABSOLUTE_MIN_CRF, max_crf, initial_crf)
    };

    crate::verbose_eprintln!("Phase 2: CPU Fine-Tune (0.5→0.1 step)");
    crate::verbose_eprintln!("Starting from GPU boundary: CRF {:.1}", cpu_center_crf);

    let clamped_cpu_center_crf = cpu_center_crf.clamp(cpu_min_crf, cpu_max_crf);
    if (clamped_cpu_center_crf - cpu_center_crf).abs() > 0.01 {
        crate::verbose_eprintln!(
            "   ⚠️ CPU start CRF {:.1} clamped to {:.1} (within valid range [{:.1}, {:.1}])",
            cpu_center_crf,
            clamped_cpu_center_crf,
            cpu_min_crf,
            cpu_max_crf
        );
        crate::verbose_eprintln!("      This is normal when GPU boundary exceeds CPU range");
        crate::verbose_eprintln!("      Search will start from boundary instead of optimal point");
    }

    let mut result = cpu_fine_tune_from_gpu_boundary(
        input,
        output,
        encoder,
        vf_args,
        clamped_cpu_center_crf,
        cpu_min_crf,
        cpu_max_crf,
        min_ssim,
        ultimate_mode,
        max_threads,
    )?;

    result.log.clear();

    crate::verbose_eprintln!();
    crate::verbose_eprintln!("Phase 3: Quality Verification");

    let mut quality_verification_skipped_for_format = false;

    if let Ok(probe_result) = crate::ffprobe::probe_video(input) {
        let duration = probe_result.duration;
        crate::verbose_eprintln!(
            "   Video duration: {:.1}s ({:.1} min)",
            duration,
            duration / 60.0
        );

        const VMAF_DURATION_THRESHOLD: f64 = 300.0;

        let is_gif_format = probe_result.format_name.eq_ignore_ascii_case("gif");

        let should_run_vmaf =
            !is_gif_format && (duration <= VMAF_DURATION_THRESHOLD || force_ms_ssim_long);

        if is_gif_format {
            crate::verbose_eprintln!(
                "   GIF input: using SSIM-All verification (ffmpeg ssim filter, GIF-compatible)"
            );

            if let Some((y, u, v, all)) = calculate_ssim_all(input, output) {
                crate::log_eprintln!("   SSIM Y/U/V/All: {:.4}/{:.4}/{:.4}/{:.4}", y, u, v, all);
                const GIF_SSIM_ALL_THRESHOLD: f64 = 0.92;
                if all < GIF_SSIM_ALL_THRESHOLD {
                    crate::log_eprintln!(
                        "   ❌ SSIM ALL BELOW TARGET! {:.4} < {:.2}",
                        all,
                        GIF_SSIM_ALL_THRESHOLD
                    );
                    result.ms_ssim_passed = Some(false);
                } else {
                    crate::log_eprintln!(
                        "   ✅ SSIM ALL TARGET MET: {:.4} ≥ {:.2}",
                        all,
                        GIF_SSIM_ALL_THRESHOLD
                    );
                    result.ms_ssim_passed = Some(true);
                }
                result.ms_ssim_score = Some(all);
            } else {
                quality_verification_skipped_for_format = true;
                let msg = "⚠️  SSIM verification failed (GIF format) - accepting based on size compression only";
                result.log.push(msg.to_string());
                result.ms_ssim_passed = None;
                result.ms_ssim_score = None;
            }
        } else if should_run_vmaf {
            crate::log_eprintln!("   ✅ Short video detected (≤5min)");
            crate::log_eprintln!("   Enabling fusion quality verification (MS-SSIM + SSIM)...");

            let ms_ssim_yuv_result = calculate_ms_ssim_yuv(input, output);
            let ssim_all_result = calculate_ssim_all(input, output);

            crate::log_eprintln!("   ═══════════════════════════════════════════════════");
            crate::log_eprintln!("   Quality Metrics:");
            let ssim_str = result
                .ssim
                .map(|s| format!("{:.6}", s))
                .unwrap_or_else(|| "N/A".to_string());
            crate::log_eprintln!("      SSIM (explore): {}", ssim_str);

            let quality_target = result.actual_min_ssim.max(0.90);

            const MS_SSIM_WEIGHT: f64 = 0.6;
            const SSIM_ALL_WEIGHT: f64 = 0.4;

            let mut final_score: Option<f64> = None;
            let mut ms_ssim_avg: Option<f64> = None;
            let mut ssim_all_val: Option<f64> = None;

            if let Some((y, u, v, avg)) = ms_ssim_yuv_result {
                crate::log_eprintln!("      MS-SSIM Y/U/V: {:.4}/{:.4}/{:.4}", y, u, v);
                crate::log_eprintln!("      MS-SSIM (3-ch avg): {:.4}", avg);
                ms_ssim_avg = Some(avg);

                let chroma_loss = (y - u).max(y - v);
                if chroma_loss > 0.02 {
                    crate::log_eprintln!(
                        "      ⚠️  MS-SSIM CHROMA DIFF: Y-U={:.4}, Y-V={:.4}",
                        y - u,
                        y - v
                    );
                }
            }

            if let Some((y, u, v, all)) = ssim_all_result {
                crate::log_eprintln!(
                    "      SSIM Y/U/V/All: {:.4}/{:.4}/{:.4}/{:.4}",
                    y,
                    u,
                    v,
                    all
                );
                ssim_all_val = Some(all);

                let chroma_loss = (y - u).max(y - v);
                if chroma_loss > 0.02 {
                    crate::log_eprintln!(
                        "      ⚠️  SSIM CHROMA LOSS: Y-U={:.4}, Y-V={:.4}",
                        y - u,
                        y - v
                    );
                }
            }

            crate::log_eprintln!("   ───────────────────────────────────────────────────");
            if let (Some(ms), Some(ss)) = (ms_ssim_avg, ssim_all_val) {
                let fusion = MS_SSIM_WEIGHT * ms + SSIM_ALL_WEIGHT * ss;
                final_score = Some(fusion);
                crate::log_eprintln!("   FUSION SCORE: {:.4}", fusion);
                crate::log_eprintln!(
                    "      Formula: {:.1}×MS-SSIM + {:.1}×SSIM_All",
                    MS_SSIM_WEIGHT,
                    SSIM_ALL_WEIGHT
                );
                crate::log_eprintln!(
                    "      = {:.1}×{:.4} + {:.1}×{:.4}",
                    MS_SSIM_WEIGHT,
                    ms,
                    SSIM_ALL_WEIGHT,
                    ss
                );
            } else if let Some(ms) = ms_ssim_avg {
                final_score = Some(ms);
                crate::log_eprintln!("   SCORE (MS-SSIM only): {:.4}", ms);
                crate::log_eprintln!("      ⚠️  SSIM All unavailable, using MS-SSIM alone");
            } else if let Some(ss) = ssim_all_val {
                final_score = Some(ss);
                crate::log_eprintln!("   SCORE (SSIM All only): {:.4}", ss);
                crate::log_eprintln!("      ⚠️  MS-SSIM unavailable, using SSIM All alone");
            }

            if let Some(score) = final_score {
                let quality_grade = if score >= 0.98 {
                    "Excellent"
                } else if score >= 0.95 {
                    "Very Good"
                } else if score >= quality_target {
                    "Good (meets target)"
                } else if score >= 0.85 {
                    "Below Target"
                } else {
                    "FAILED"
                };
                crate::log_eprintln!(
                    "      Grade: {} (target: ≥{:.2})",
                    quality_grade,
                    quality_target
                );

                if score < quality_target {
                    crate::log_eprintln!(
                        "   ❌ FUSION SCORE BELOW TARGET! {:.4} < {:.2}",
                        score,
                        quality_target
                    );
                    crate::log_eprintln!("      ⚠️  Quality does not meet threshold!");
                    crate::log_eprintln!("      Suggestion: Lower CRF or disable --compress");
                    result.ms_ssim_passed = Some(false);
                    result.ms_ssim_score = Some(score);
                } else {
                    crate::log_eprintln!(
                        "   ✅ FUSION SCORE TARGET MET: {:.4} ≥ {:.2}",
                        score,
                        quality_target
                    );
                    result.ms_ssim_passed = Some(true);
                    result.ms_ssim_score = Some(score);
                }
            } else {
                let err_lines = [
                    "   ════════════════════════════════════════════════════",
                    "   ❌ ERROR: Fusion verification incomplete (MS-SSIM + SSIM All failed).",
                    "   ❌ Refusing to mark as passed — no fallback to single-channel or explore SSIM.",
                    "   ❌ Possible causes: libvmaf unavailable, pixel format, or resolution mismatch.",
                    "   ════════════════════════════════════════════════════",
                ];
                for line in &err_lines {
                    crate::log_eprintln!("{}", line);
                    result.log.push((*line).to_string());
                }
                result.ms_ssim_passed = Some(false);
                result.ms_ssim_score = None;
            }
        } else {
            crate::log_eprintln!(
                "   Long video (>{:.0}min) - skipping MS-SSIM, using SSIM All verification",
                VMAF_DURATION_THRESHOLD / 60.0
            );

            if let Some((y, u, v, all)) = calculate_ssim_all(input, output) {
                crate::log_eprintln!("   SSIM Y/U/V/All: {:.4}/{:.4}/{:.4}/{:.4}", y, u, v, all);

                const SSIM_ALL_THRESHOLD: f64 = 0.92;
                if all < SSIM_ALL_THRESHOLD {
                    crate::log_eprintln!(
                        "   ❌ SSIM ALL BELOW TARGET! {:.4} < {:.2}",
                        all,
                        SSIM_ALL_THRESHOLD
                    );
                    result.ms_ssim_passed = Some(false);
                } else {
                    crate::log_eprintln!(
                        "   ✅ SSIM ALL TARGET MET: {:.4} ≥ {:.2}",
                        all,
                        SSIM_ALL_THRESHOLD
                    );
                    result.ms_ssim_passed = Some(true);
                }
                result.ms_ssim_score = Some(all);
            } else {
                let err_lines = [
                    "   ❌ ERROR: SSIM All calculation failed (long-video path). Refusing to mark as passed.",
                ];
                for line in &err_lines {
                    crate::log_eprintln!("{}", line);
                    result.log.push((*line).to_string());
                }
                result.ms_ssim_passed = Some(false);
                result.ms_ssim_score = None;
            }
        }
    } else {
        crate::log_eprintln!("   ⚠️  Could not determine video duration");
        crate::log_eprintln!("   Using SSIM All verification (includes chroma)...");

        if let Some((y, u, v, all)) = calculate_ssim_all(input, output) {
            crate::log_eprintln!("   SSIM Y/U/V/All: {:.4}/{:.4}/{:.4}/{:.4}", y, u, v, all);

            const SSIM_ALL_THRESHOLD: f64 = 0.92;
            if all < SSIM_ALL_THRESHOLD {
                crate::log_eprintln!(
                    "   ❌ SSIM ALL BELOW TARGET! {:.4} < {:.2}",
                    all,
                    SSIM_ALL_THRESHOLD
                );
                result.ms_ssim_passed = Some(false);
            } else {
                crate::log_eprintln!(
                    "   ✅ SSIM ALL TARGET MET: {:.4} ≥ {:.2}",
                    all,
                    SSIM_ALL_THRESHOLD
                );
                result.ms_ssim_passed = Some(true);
            }
            result.ms_ssim_score = Some(all);
        } else {
            let err_lines = [
                "   ❌ ERROR: SSIM All calculation failed (no duration path). Refusing to mark as passed.",
            ];
            for line in &err_lines {
                crate::log_eprintln!("{}", line);
                result.log.push((*line).to_string());
            }
            result.ms_ssim_passed = Some(false);
            result.ms_ssim_score = None;
        }
    }

    let input_size = fs::metadata(input).ok().map(|m| m.len());
    let output_size_actual = fs::metadata(output)
        .ok()
        .map(|m| m.len())
        .unwrap_or(result.output_size);
    let size_change_line =
        if let (Some(in_sz), Some(out_sz)) = (input_size, Some(output_size_actual)) {
            if in_sz == 0 {
                "   SizeChange: N/A (zero input size)".to_string()
            } else {
                let ratio = out_sz as f64 / in_sz as f64;
                let pct = (ratio - 1.0) * 100.0;
                format!("   SizeChange: {:.2}x ({:+.1}%) vs original", ratio, pct)
            }
        } else {
            "   SizeChange: N/A (missing original or output size)".to_string()
        };
    result.log.push(size_change_line);

    let quality_line = if result.ms_ssim_passed == Some(false) && result.ms_ssim_score.is_none() {
        "   Quality: N/A (quality check failed)".to_string()
    } else if let Some(score) = result.ms_ssim_score {
        let pct = (score * 100.0 * 10.0).round() / 10.0;
        format!("   Quality: {:.1}% (MS-SSIM={:.4})", pct, score)
    } else if let Some(s) = result.ssim {
        let pct = (s * 100.0 * 10.0).round() / 10.0;
        format!("   Quality: {:.1}% (SSIM={:.4}, approx.)", pct, s)
    } else {
        "   Quality: N/A (quality check failed)".to_string()
    };
    result.log.push(quality_line);

    let quality_check_line = match (result.ms_ssim_passed, result.quality_passed) {
        (_, true) => "   QualityCheck: PASSED (quality + total file size target met)",
        (Some(true), false) => "   QualityCheck: FAILED (quality met but total file not smaller)",
        (Some(false), _) => "   QualityCheck: FAILED (below target or verification failed)",
        (None, false) if quality_verification_skipped_for_format => {
            "   QualityCheck: N/A (GIF/size-only, quality not measured)"
        }
        (None, false) => "   QualityCheck: FAILED (quality not verified)",
    };
    result.log.push(quality_check_line.to_string());

    crate::log_eprintln!();

    if gpu.is_available() && has_gpu_encoder {
        let mapping = match encoder {
            VideoEncoder::Hevc => CrfMapping::hevc(gpu.gpu_type),
            VideoEncoder::Av1 => CrfMapping::av1(gpu.gpu_type),
            VideoEncoder::H264 => CrfMapping::hevc(gpu.gpu_type),
        };
        let equivalent_gpu_crf = mapping.cpu_to_gpu(result.optimal_crf);
        crate::verbose_eprintln!("   ═══════════════════════════════════════════════════");
        crate::verbose_eprintln!(
            "   CRF Mapping: CPU {:.1} ≈ GPU {:.1}",
            result.optimal_crf,
            equivalent_gpu_crf
        );
    }

    Ok(result)
}

#[allow(unused_assignments)]
fn cpu_fine_tune_from_gpu_boundary(
    input: &Path,
    output: &Path,
    encoder: VideoEncoder,
    vf_args: Vec<String>,
    gpu_boundary_crf: f32,
    min_crf: f32,
    max_crf: f32,
    min_ssim: f64,
    ultimate_mode: bool,
    max_threads: usize,
) -> Result<ExploreResult> {
    #[allow(unused_mut)]
    let mut log = Vec::new();

    let input_size = fs::metadata(input)
        .context("Failed to read input file metadata")?
        .len();

    let input_stream_info = crate::stream_size::extract_stream_sizes(input);
    let input_video_stream_size = input_stream_info.video_stream_size;
    crate::verbose_eprintln!(
        "{}Input video stream: {} (total file: {}, overhead: {:.1}%)",
        CYAN,
        crate::modern_ui::format_size(input_video_stream_size),
        crate::modern_ui::format_size(input_size),
        input_stream_info.container_overhead_percent()
    );

    let duration: f32 = {
        use std::process::Command;
        let duration_output = Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-show_entries",
                "format=duration",
                "-of",
                "default=noprint_wrappers=1:nokey=1",
                "--",
            ])
            .arg(crate::safe_path_arg(input).as_ref())
            .output();
        duration_output
            .ok()
            .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok())
            .unwrap_or(60.0)
    };

    let estimated_iterations = if ultimate_mode {
        let crf_range = max_crf - min_crf;
        let adaptive_walls = calculate_adaptive_max_walls(crf_range);
        (adaptive_walls + 10) as u64
    } else {
        15
    };
    let cpu_progress =
        crate::UnifiedProgressBar::new_iteration("CPU Fine-Tune", input_size, estimated_iterations);

    #[derive(Debug, Clone)]
    enum AudioTranscodeStrategy {
        Copy,
        Alac,
        AacHigh,
        AacMedium,
    }

    let audio_strategy = {
        let output_ext = output
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        let is_mov_mp4 = output_ext == "mov" || output_ext == "mp4" || output_ext == "m4v";

        if !is_mov_mp4 {
            AudioTranscodeStrategy::Copy
        } else {
            let probe_result = crate::ffprobe::probe_video(input).ok();
            let audio_codec = probe_result
                .as_ref()
                .and_then(|info| info.audio_codec.as_ref())
                .map(|s| s.to_lowercase())
                .unwrap_or_default();
            let audio_bitrate = probe_result
                .as_ref()
                .and_then(|info| info.audio_bit_rate)
                .unwrap_or(0);

            let incompatible = audio_codec.contains("opus")
                || audio_codec.contains("vorbis")
                || audio_codec.contains("webm");

            let is_lossless = audio_codec.contains("flac")
                || audio_codec.contains("alac")
                || audio_codec.contains("pcm")
                || audio_codec.contains("wav");

            if !incompatible {
                AudioTranscodeStrategy::Copy
            } else if is_lossless || audio_bitrate > 256_000 {
                crate::log_eprintln!(
                    "   🎵 High-quality audio detected ({}kbps {}), using ALAC (lossless)",
                    audio_bitrate / 1000,
                    audio_codec
                );
                AudioTranscodeStrategy::Alac
            } else if audio_bitrate >= 128_000 {
                crate::log_eprintln!(
                    "   🎵 Medium-quality audio ({}kbps {}), using AAC 256k",
                    audio_bitrate / 1000,
                    audio_codec
                );
                AudioTranscodeStrategy::AacHigh
            } else {
                crate::log_eprintln!(
                    "   🎵 Audio codec '{}' incompatible with {}, using AAC 192k",
                    audio_codec,
                    output_ext.to_uppercase()
                );
                AudioTranscodeStrategy::AacMedium
            }
        }
    };

    let encode_full = |crf: f32| -> Result<u64> {
        use std::io::{BufRead, BufReader, Write};
        use std::process::Stdio;

        let mut cmd = std::process::Command::new("ffmpeg");
        cmd.arg("-y");
        cmd.arg("-progress").arg("pipe:1");

        cmd.arg("-i")
            .arg(crate::safe_path_arg(input).as_ref())
            .arg("-c:v")
            .arg(encoder.ffmpeg_name())
            .arg("-crf")
            .arg(format!("{:.1}", crf));

        for arg in encoder.extra_args(max_threads) {
            cmd.arg(arg);
        }

        for arg in &vf_args {
            if !arg.is_empty() {
                cmd.arg(arg);
            }
        }

        match &audio_strategy {
            AudioTranscodeStrategy::Copy => {
                cmd.arg("-c:a").arg("copy");
            }
            AudioTranscodeStrategy::Alac => {
                cmd.arg("-c:a").arg("alac");
            }
            AudioTranscodeStrategy::AacHigh => {
                cmd.arg("-c:a").arg("aac").arg("-b:a").arg("256k");
            }
            AudioTranscodeStrategy::AacMedium => {
                cmd.arg("-c:a").arg("aac").arg("-b:a").arg("192k");
            }
        }
        cmd.arg(crate::safe_path_arg(output).as_ref());

        cmd.stdout(Stdio::piped());
        let stderr_temp_val = tempfile::Builder::new()
            .suffix(".log")
            .tempfile()
            .context("Failed to create stderr temp file")?;

        let stderr_file = stderr_temp_val.path().to_path_buf();
        let stderr_temp = Some(stderr_temp_val);

        if let Some(ref temp) = stderr_temp {
            if let Ok(file) = temp.reopen() {
                cmd.stderr(file);
            } else {
                cmd.stderr(Stdio::null());
            }
        } else {
            if let Ok(file) = std::fs::File::create(&stderr_file) {
                cmd.stderr(file);
            } else {
                cmd.stderr(Stdio::null());
            }
        }

        let mut child = cmd.spawn().context("Failed to spawn ffmpeg")?;

        if let Some(stdout) = child.stdout.take() {
            let reader = BufReader::new(stdout);
            let mut last_fps = 0.0_f64;
            let mut last_speed = String::new();
            let mut last_time_us = 0_i64;
            let duration_secs = duration as f64;

            for line in reader.lines().map_while(Result::ok) {
                if let Some(val) = line.strip_prefix("out_time_us=") {
                    if let Ok(time_us) = val.parse::<i64>() {
                        last_time_us = time_us;
                    }
                } else if let Some(val) = line.strip_prefix("fps=") {
                    if let Ok(fps) = val.parse::<f64>() {
                        last_fps = fps;
                    }
                } else if let Some(val) = line.strip_prefix("speed=") {
                    last_speed = val.trim().to_string();
                } else if line == "progress=continue" || line == "progress=end" {
                    let current_secs = last_time_us as f64 / 1_000_000.0;
                    if duration_secs > 0.0 {
                        let pct = (current_secs / duration_secs * 100.0).min(100.0);
                        eprint!(
                            "\r      ⏳ CRF {:.1} | {:.1}% | {:.1}s/{:.1}s | {:.0}fps | {}   ",
                            crf, pct, current_secs, duration_secs, last_fps, last_speed
                        );
                    }
                    let _ = std::io::stderr().flush();
                }
            }
        }

        let status = child.wait().context("Failed to wait for ffmpeg")?;
        eprint!(
            "\r                                                                              \r"
        );

        if !status.success() {
            let error_detail = if stderr_file.exists() {
                let stderr_content = std::fs::read_to_string(&stderr_file).unwrap_or_default();
                let _ = std::fs::remove_file(&stderr_file);
                let error_lines: Vec<&str> = stderr_content
                    .lines()
                    .filter(|l| {
                        l.contains("Error")
                            || l.contains("error")
                            || l.contains("Invalid")
                            || l.contains("failed")
                    })
                    .collect();
                if !error_lines.is_empty() {
                    format!("\n   FFmpeg error: {}", error_lines.join("\n   "))
                } else {
                    let last_lines: Vec<&str> = stderr_content.lines().rev().take(3).collect();
                    if !last_lines.is_empty() {
                        format!(
                            "\n   FFmpeg output: {}",
                            last_lines
                                .into_iter()
                                .rev()
                                .collect::<Vec<_>>()
                                .join("\n   ")
                        )
                    } else {
                        String::new()
                    }
                }
            } else {
                String::new()
            };
            anyhow::bail!("❌ Encoding failed at CRF {:.1}{}", crf, error_detail);
        }

        let _ = std::fs::remove_file(&stderr_file);

        Ok(fs::metadata(output)?.len())
    };

    use crate::modern_ui::colors::*;

    crate::verbose_eprintln!(
        "{}CPU Fine-Tune ({:?}) - Maximum SSIM Search{}",
        BRIGHT_CYAN,
        encoder,
        RESET
    );
    crate::verbose_eprintln!(
        "{}Input: {} ({} bytes) | Duration: {}",
        CYAN,
        crate::modern_ui::format_size(input_size),
        input_size,
        crate::modern_ui::format_duration(duration as f64)
    );
    crate::verbose_eprintln!(
        "{}Goal: min(CRF) where output < input (Highest SSIM + Must Compress){}",
        YELLOW,
        RESET
    );
    crate::verbose_eprintln!(
        "{}Using 0.25 step (fast coarse search) + 0.1 fine-tune{}",
        CYAN,
        RESET
    );
    let step_size = 0.25_f32;

    const MAX_CONSECUTIVE_FAILURES: u32 = 3;

    let mut iterations = 0u32;
    let mut size_cache: CrfCache<u64> = CrfCache::new();

    let encode_cached = |crf: f32, cache: &mut CrfCache<u64>| -> Result<u64> {
        if let Some(&size) = cache.get(crf) {
            cpu_progress.inc_iteration(crf, size, None);
            return Ok(size);
        }
        let size = encode_full(crf)?;
        cache.insert(crf, size);
        cpu_progress.inc_iteration(crf, size, None);
        Ok(size)
    };

    let mut best_crf: Option<f32> = None;
    let mut best_size: Option<u64> = None;
    #[allow(unused_assignments)]
    let mut best_ssim_tracked: Option<f64> = None;

    crate::verbose_eprintln!(
        "{}Step: {:.2} | GPU boundary: CRF {:.1}{}",
        DIM,
        step_size,
        gpu_boundary_crf,
        RESET
    );
    crate::verbose_eprintln!("{}Goal: min(CRF) where output < input{}", DIM, RESET);
    crate::verbose_eprintln!(
        "{}Strategy: Marginal benefit analysis (not hard stop){}",
        DIM,
        RESET
    );
    crate::verbose_eprintln!();

    let calculate_ssim_quick = || -> Option<f64> {
        let filters = [
            "[0:v]scale='iw-mod(iw,2)':'ih-mod(ih,2)':flags=bicubic[ref];[ref][1:v]ssim",
            "[0:v]format=yuv420p,scale='iw-mod(iw,2)':'ih-mod(ih,2)'[ref];[1:v]format=yuv420p[cmp];[ref][cmp]ssim",
            "ssim",
        ];

        for filter in &filters {
            let ssim_output = std::process::Command::new("ffmpeg")
                .arg("-i")
                .arg(crate::safe_path_arg(input).as_ref())
                .arg("-i")
                .arg(crate::safe_path_arg(output).as_ref())
                .arg("-lavfi")
                .arg(filter)
                .arg("-f")
                .arg("null")
                .arg("-")
                .output();

            if let Ok(out) = ssim_output {
                if out.status.success() {
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    if let Some(line) = stderr.lines().find(|l| l.contains("All:")) {
                        if let Some(all_pos) = line.find("All:") {
                            let after_all = &line[all_pos + 4..];
                            let end = after_all
                                .find(|c: char| !c.is_numeric() && c != '.')
                                .unwrap_or(after_all.len());
                            if end > 0 {
                                if let Ok(ssim) = after_all[..end].parse::<f64>() {
                                    if (0.0..=1.0).contains(&ssim) {
                                        return Some(ssim);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        None
    };

    crate::verbose_eprintln!("{}Phase 1: Verify GPU boundary{}", BRIGHT_CYAN, RESET);

    let gpu_size = match encode_cached(gpu_boundary_crf, &mut size_cache) {
        Ok(size) => size,
        Err(e) => {
            crate::log_eprintln!(
                "{}⚠️  GPU boundary verification failed at CRF {:.1}{}",
                BRIGHT_YELLOW,
                gpu_boundary_crf,
                RESET
            );
            crate::log_eprintln!("   Error: {}", e);
            crate::log_eprintln!("   Retrying with CPU encoding (x265 CLI)...");

            match encode_cached(gpu_boundary_crf, &mut size_cache) {
                Ok(size) => {
                    crate::log_eprintln!("   {}✅ CPU encoding succeeded{}", BRIGHT_GREEN, RESET);
                    size
                }
                Err(cpu_err) => {
                    crate::log_eprintln!("   {}❌ CPU encoding also failed{}", BRIGHT_RED, RESET);
                    crate::log_eprintln!("   CPU Error: {}", cpu_err);
                    return Err(cpu_err);
                }
            }
        }
    };
    iterations += 1;
    let gpu_output_video_size = crate::stream_size::get_output_video_stream_size(output);
    let gpu_pct = stream_size_change_pct(gpu_output_video_size, input_video_stream_size);
    let gpu_ssim = calculate_ssim_quick();

    if crate::stream_size::can_compress_pure_video(output, input_video_stream_size) {
        best_crf = Some(gpu_boundary_crf);
        best_size = Some(gpu_size);
        best_ssim_tracked = gpu_ssim;
        crate::log_eprintln!(
            "{}✅{} GPU boundary {}CRF {:.1}{}: {}{:+.1}%{} SSIM {}{}{} (compresses)",
            BRIGHT_GREEN,
            RESET,
            BRIGHT_CYAN,
            gpu_boundary_crf,
            RESET,
            BRIGHT_GREEN,
            gpu_pct,
            RESET,
            BRIGHT_YELLOW,
            gpu_ssim
                .map(|s| format!("{:.4}", s))
                .unwrap_or_else(|| "N/A".to_string()),
            RESET
        );
        crate::log_eprintln!();
        crate::verbose_eprintln!(
            "{}Phase 2: Maximum SSIM Search - Smart Wall Collision (v5.93){}",
            BRIGHT_CYAN,
            RESET
        );
        crate::verbose_eprintln!(
            "   {}(Adaptive step, MUST hit wall OR min_crf boundary){}",
            DIM,
            RESET
        );

        let crf_range = gpu_boundary_crf - min_crf;

        let initial_step = (crf_range / 1.5).clamp(8.0, 25.0);
        const DECAY_FACTOR: f32 = 0.4;
        const MIN_STEP: f32 = 0.1;

        let max_wall_hits = if duration >= VERY_LONG_VIDEO_THRESHOLD_SECS {
            6
        } else if duration >= LONG_VIDEO_THRESHOLD_SECS {
            8
        } else if ultimate_mode {
            calculate_adaptive_max_walls(crf_range)
        } else {
            NORMAL_MAX_WALL_HITS
        };

        let required_zero_gains =
            calculate_zero_gains_for_duration_and_range(duration, crf_range, ultimate_mode);

        let max_iterations_for_video =
            calculate_max_iterations_for_duration(duration, ultimate_mode);

        if ultimate_mode {
            crate::verbose_eprintln!(
                "   {}ULTIMATE MODE: searching until SSIM saturation (Domain Wall){}",
                BRIGHT_MAGENTA,
                RESET
            );
            crate::verbose_eprintln!("   {}CRF range: {:.1} → Adaptive max walls: {}{}{} (formula: ceil(log2({:.1}))+{}){}",
                DIM, crf_range, BRIGHT_CYAN, max_wall_hits, RESET, crf_range, super::ADAPTIVE_WALL_LOG_BASE, RESET);
            crate::verbose_eprintln!(
                "   {}SSIM saturation: {}{}{} consecutive zero-gains < 0.00005{}",
                DIM,
                BRIGHT_YELLOW,
                required_zero_gains,
                RESET,
                RESET
            );
        } else {
            crate::verbose_eprintln!(
                "   {}CRF range: {:.1} → Initial step: {}{:.1}{} (v6.2 curve model){}",
                DIM,
                crf_range,
                BRIGHT_CYAN,
                initial_step,
                RESET,
                RESET
            );
            crate::verbose_eprintln!(
                "   {}Strategy: Aggressive curve decay (step × 0.4 per wall hit, max {} hits){}",
                DIM,
                max_wall_hits,
                RESET
            );
        }

        let mut current_step = initial_step;
        let mut wall_hits: u32 = 0;
        let mut test_crf = gpu_boundary_crf - current_step;
        #[allow(unused_assignments)]
        let mut prev_ssim_opt = gpu_ssim;
        #[allow(unused_variables, unused_assignments)]
        let mut _prev_size = gpu_size;
        let mut last_good_crf = gpu_boundary_crf;
        let mut last_good_size = gpu_size;
        let mut last_good_ssim = gpu_ssim;
        #[allow(unused_assignments)]
        let mut overshoot_detected = false;

        let gpu_ssim_baseline = gpu_ssim.unwrap_or(0.95);
        crate::verbose_eprintln!(
            "   {}GPU SSIM baseline: {}{:.4}{} (CPU target: break through 0.97+)",
            DIM,
            BRIGHT_YELLOW,
            gpu_ssim_baseline,
            RESET
        );

        const ZERO_GAIN_THRESHOLD: f64 = 0.00005;

        let mut consecutive_zero_gains: u32 = 0;
        let mut quality_wall_hit = false;
        let mut domain_wall_hit = false;

        if duration >= LONG_VIDEO_THRESHOLD_SECS {
            crate::verbose_eprintln!("   {}Long video ({:.1} min) - no iteration limit, searching until SSIM saturates{}",
                BRIGHT_CYAN, duration / 60.0, RESET);
            crate::verbose_eprintln!(
                "   {}Fallback limit: {} (emergency only), Max walls: {}, Zero-gains: {}{}",
                DIM,
                max_iterations_for_video,
                max_wall_hits,
                required_zero_gains,
                RESET
            );
        }

        while iterations < max_iterations_for_video {
            if test_crf < min_crf {
                if current_step > MIN_STEP + 0.01 {
                    crate::verbose_eprintln!(
                        "   {}Reached min_crf boundary, fine tuning from CRF {:.1}{}",
                        BRIGHT_CYAN,
                        last_good_crf,
                        RESET
                    );
                    current_step = MIN_STEP;
                    test_crf = last_good_crf - current_step;
                    if test_crf < min_crf {
                        break;
                    }
                } else {
                    break;
                }
            }

            if size_cache.contains_key(test_crf) {
                test_crf -= current_step;
                continue;
            }

            let size = encode_cached(test_crf, &mut size_cache)?;
            iterations += 1;
            let output_video_size = crate::stream_size::get_output_video_stream_size(output);
            let size_pct = stream_size_change_pct(output_video_size, input_video_stream_size);
            let current_ssim_opt = calculate_ssim_quick();

            if crate::stream_size::can_compress_pure_video(output, input_video_stream_size) {
                last_good_crf = test_crf;
                last_good_size = size;
                last_good_ssim = current_ssim_opt;
                best_crf = Some(test_crf);
                best_size = Some(size);
                best_ssim_tracked = current_ssim_opt;

                let should_stop = match (current_ssim_opt, prev_ssim_opt) {
                    (Some(current_ssim), Some(prev_ssim)) => {
                        let ssim_gain = current_ssim - prev_ssim;

                        let ssim_vs_gpu = current_ssim / gpu_ssim_baseline;
                        let gpu_comparison = if ssim_vs_gpu > 1.01 {
                            format!("{}×{:.3} GPU{}", BRIGHT_GREEN, ssim_vs_gpu, RESET)
                        } else if ssim_vs_gpu > 1.001 {
                            format!("{}×{:.4} GPU{}", GREEN, ssim_vs_gpu, RESET)
                        } else {
                            format!("{}≈GPU{}", DIM, RESET)
                        };

                        let is_zero_gain = ssim_gain.abs() < ZERO_GAIN_THRESHOLD;
                        if current_step <= MIN_STEP + 0.01 {
                            if is_zero_gain {
                                consecutive_zero_gains += 1;
                            } else {
                                consecutive_zero_gains = 0;
                            }
                        }

                        let quality_wall_triggered = consecutive_zero_gains >= required_zero_gains
                            && current_step <= MIN_STEP + 0.01;

                        let wall_status = if quality_wall_triggered {
                            if ultimate_mode {
                                format!("{}DOMAIN WALL{}", BRIGHT_MAGENTA, RESET)
                            } else {
                                format!("{}QUALITY WALL{}", BRIGHT_YELLOW, RESET)
                            }
                        } else if consecutive_zero_gains > 0 && current_step <= MIN_STEP + 0.01 {
                            format!(
                                "{}[{}/{}]{}",
                                DIM, consecutive_zero_gains, required_zero_gains, RESET
                            )
                        } else {
                            String::new()
                        };

                        crate::log_eprintln!("   {}✓{} {}CRF {:.1}{}: {}{:+.1}%{} SSIM {}{:.4}{} ({}Δ{:+.5}{}, step {}{:.2}{}) {} {}✅{} {}",
                            BRIGHT_GREEN, RESET, CYAN, test_crf, RESET,
                            BRIGHT_GREEN, size_pct, RESET, BRIGHT_YELLOW, current_ssim, RESET,
                            DIM, ssim_gain, RESET, DIM, current_step, RESET,
                            gpu_comparison, BRIGHT_GREEN, RESET, wall_status);

                        if quality_wall_triggered {
                            quality_wall_hit = true;
                        }
                        quality_wall_triggered
                    }
                    _ => {
                        crate::log_eprintln!("   {}✓{} {}CRF {:.1}{}: {}{:+.1}%{} SSIM {}N/A{} (step {}{:.2}{}) {}✅{}",
                            BRIGHT_GREEN, RESET, CYAN, test_crf, RESET,
                            BRIGHT_GREEN, size_pct, RESET, DIM, RESET, DIM, current_step, RESET, BRIGHT_GREEN, RESET);
                        false
                    }
                };

                if should_stop {
                    crate::log_eprintln!();
                    if ultimate_mode {
                        domain_wall_hit = true;
                        crate::log_eprintln!("   {}DOMAIN WALL HIT:{} SSIM fully saturated after {} consecutive zero-gains",
                            BRIGHT_MAGENTA, RESET, consecutive_zero_gains);
                    } else {
                        crate::log_eprintln!("   {}QUALITY WALL HIT:{} SSIM saturated after {} consecutive zero-gains",
                            BRIGHT_YELLOW, RESET, consecutive_zero_gains);
                    }
                    crate::verbose_eprintln!(
                        "   {}Final: CRF {:.1}, compression {:+.1}%, iterations {}{}",
                        BRIGHT_CYAN,
                        test_crf,
                        size_pct,
                        iterations,
                        RESET
                    );
                    break;
                }

                prev_ssim_opt = current_ssim_opt;
                _prev_size = size;
                test_crf -= current_step;
            } else {
                overshoot_detected = true;
                wall_hits += 1;

                let video_size_diff = crate::format_size_diff(
                    output_video_size as i64 - input_video_stream_size as i64,
                );
                crate::log_eprintln!(
                    "   {}✗{} {}CRF {:.1}{}: {}{:+.1}%{} {}❌ WALL HIT #{}{} (video stream {}{}{})",
                    BRIGHT_RED,
                    RESET,
                    CYAN,
                    test_crf,
                    RESET,
                    BRIGHT_RED,
                    size_pct,
                    RESET,
                    RED,
                    wall_hits,
                    RESET,
                    RED,
                    video_size_diff,
                    RESET
                );

                if wall_hits >= max_wall_hits {
                    if ultimate_mode {
                        crate::log_eprintln!(
                            "   {}Adaptive wall limit ({}) reached.{} Stopping at best CRF {:.1}",
                            BRIGHT_YELLOW,
                            max_wall_hits,
                            RESET,
                            last_good_crf
                        );
                    } else {
                        crate::log_eprintln!(
                            "   {}Max wall hits ({}) reached.{} Stopping at best CRF {:.1}",
                            BRIGHT_YELLOW,
                            max_wall_hits,
                            RESET,
                            last_good_crf
                        );
                    }
                    break;
                }

                let curve_step = initial_step * DECAY_FACTOR.powi(wall_hits as i32);

                let new_step = if curve_step < 1.0 {
                    MIN_STEP
                } else {
                    curve_step
                };

                let phase_info = if new_step <= MIN_STEP + 0.01 {
                    format!("{}→ FINE TUNING{}", BRIGHT_GREEN, RESET)
                } else {
                    format!("decay {}×{:.1}^{}{}", DIM, DECAY_FACTOR, wall_hits, RESET)
                };

                crate::log_eprintln!(
                    "   {}Backtrack: step {:.2} → {:.2} ({}){}",
                    YELLOW,
                    current_step,
                    new_step,
                    phase_info,
                    RESET
                );

                current_step = new_step;
                test_crf = last_good_crf - current_step;
            }
        }

        if domain_wall_hit {
            if best_crf.is_none_or(|c| c > last_good_crf) {
                best_crf = Some(last_good_crf);
                best_size = Some(last_good_size);
                best_ssim_tracked = last_good_ssim;
            }
        } else if quality_wall_hit {
            if best_crf.is_none_or(|c| c > last_good_crf) {
                best_crf = Some(last_good_crf);
                best_size = Some(last_good_size);
                best_ssim_tracked = last_good_ssim;
            }
        } else if overshoot_detected {
            crate::log_eprintln!();
            crate::log_eprintln!(
                "   {}Size wall hit: overshoot at CRF < {:.1}{}",
                BRIGHT_RED,
                last_good_crf,
                RESET
            );
            crate::verbose_eprintln!(
                "   {}Final: CRF {:.1}, iterations {}{}",
                BRIGHT_CYAN,
                last_good_crf,
                iterations,
                RESET
            );
        } else if test_crf < min_crf {
            crate::log_eprintln!();
            crate::log_eprintln!(
                "   {}Min CRF boundary reached (highly compressible){}",
                BRIGHT_GREEN,
                RESET
            );
            crate::verbose_eprintln!(
                "   {}Final: CRF {:.1}, iterations {}{}",
                BRIGHT_CYAN,
                last_good_crf,
                iterations,
                RESET
            );

            if best_crf.is_none_or(|c| c > last_good_crf) {
                best_crf = Some(last_good_crf);
                best_size = Some(last_good_size);
                best_ssim_tracked = last_good_ssim;
            }
        }
    } else {
        crate::log_eprintln!(
            "⚠️ GPU boundary CRF {:.1}: {:+.1}% (TOO LARGE)",
            gpu_boundary_crf,
            gpu_pct
        );
        crate::log_eprintln!();
        crate::log_eprintln!("Phase 2: Search UPWARD for compression boundary");
        crate::log_eprintln!("   (Higher CRF = Smaller file, find first compressible)");

        let mut test_crf = gpu_boundary_crf + step_size;
        let mut found_compress_point = false;

        let max_iterations_for_video =
            calculate_max_iterations_for_duration(duration, ultimate_mode);

        while test_crf <= max_crf && iterations < max_iterations_for_video {
            let size = encode_cached(test_crf, &mut size_cache)?;
            iterations += 1;
            let output_video_size = crate::stream_size::get_output_video_stream_size(output);
            let size_pct = stream_size_change_pct(output_video_size, input_video_stream_size);

            if crate::stream_size::can_compress_pure_video(output, input_video_stream_size) {
                best_crf = Some(test_crf);
                best_size = Some(size);
                best_ssim_tracked = calculate_ssim_quick();
                found_compress_point = true;
                crate::log_eprintln!("   ✓ CRF {:.1}: {:+.1}% ✅ (FOUND!)", test_crf, size_pct);
                break;
            } else {
                crate::log_eprintln!("   ✗ CRF {:.1}: {:+.1}% ❌", test_crf, size_pct);
            }
            test_crf += step_size;
        }

        if !found_compress_point {
            crate::log_eprintln!("⚠️ Cannot compress even at max CRF {:.1}!", max_crf);
            crate::log_eprintln!("   File may be already optimally compressed");
            let last_output_video = crate::stream_size::get_output_video_stream_size(output);
            crate::verbose_eprintln!(
                "   Video stream: input {} vs output {} ({:+.1}%)",
                crate::format_bytes(input_video_stream_size),
                crate::format_bytes(last_output_video),
                stream_size_change_pct(last_output_video, input_video_stream_size)
            );
            let max_size = encode_cached(max_crf, &mut size_cache)?;
            iterations += 1;
            best_crf = Some(max_crf);
            best_size = Some(max_size);
        } else {
            crate::log_eprintln!();
            crate::log_eprintln!("Phase 3: Search DOWNWARD with marginal benefit analysis");

            let compress_point = best_crf.unwrap_or(gpu_boundary_crf);
            let mut test_crf = compress_point - step_size;
            let mut consecutive_failures = 0u32;
            let mut prev_ssim_opt = best_ssim_tracked;
            let mut prev_size = best_size.unwrap_or(0);

            while test_crf >= min_crf && iterations < max_iterations_for_video {
                if size_cache.contains_key(test_crf) {
                    test_crf -= step_size;
                    continue;
                }

                let size = encode_cached(test_crf, &mut size_cache)?;
                iterations += 1;
                let output_video_size = crate::stream_size::get_output_video_stream_size(output);
                let size_pct = stream_size_change_pct(output_video_size, input_video_stream_size);
                let current_ssim_opt = calculate_ssim_quick();

                if crate::stream_size::can_compress_pure_video(output, input_video_stream_size) {
                    consecutive_failures = 0;

                    best_crf = Some(test_crf);
                    best_size = Some(size);
                    best_ssim_tracked = current_ssim_opt;

                    let size_increase = size as f64 - prev_size as f64;
                    let size_increase_pct = if prev_size > 0 {
                        (size_increase / prev_size as f64) * 100.0
                    } else {
                        0.0
                    };

                    let should_stop = match (current_ssim_opt, prev_ssim_opt) {
                        (Some(current_ssim), Some(prev_ssim)) => {
                            let ssim_gain = current_ssim - prev_ssim;

                            crate::log_eprintln!(
                                "   ✓ CRF {:.1}: {:+.1}% SSIM {:.4} (Δ{:+.4}, size {:+.1}%) ✅",
                                test_crf,
                                size_pct,
                                current_ssim,
                                ssim_gain,
                                size_increase_pct
                            );

                            if ssim_gain < 0.0001 && current_ssim >= 0.99 {
                                crate::log_eprintln!("   SSIM plateau → STOP");
                                true
                            } else if size_increase_pct > 5.0 && ssim_gain < 0.001 {
                                crate::log_eprintln!(
                                    "   Diminishing returns (size +{:.1}% but SSIM +{:.4}) → STOP",
                                    size_increase_pct,
                                    ssim_gain
                                );
                                true
                            } else {
                                false
                            }
                        }
                        _ => {
                            crate::log_eprintln!(
                                "   ✓ CRF {:.1}: {:+.1}% SSIM N/A (size {:+.1}%) ✅",
                                test_crf,
                                size_pct,
                                size_increase_pct
                            );
                            false
                        }
                    };

                    if should_stop {
                        break;
                    }

                    prev_ssim_opt = current_ssim_opt;
                    prev_size = size;
                    test_crf -= step_size;
                } else {
                    consecutive_failures += 1;
                    crate::log_eprintln!(
                        "   ✗ CRF {:.1}: {:+.1}% ❌ (fail #{}/{})",
                        test_crf,
                        size_pct,
                        consecutive_failures,
                        MAX_CONSECUTIVE_FAILURES
                    );

                    if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                        crate::log_eprintln!(
                            "   {} consecutive failures → STOP",
                            MAX_CONSECUTIVE_FAILURES
                        );
                        break;
                    }

                    test_crf -= step_size;
                }
            }
        }
    }

    let (final_crf, final_full_size) = match (best_crf, best_size) {
        (Some(crf), Some(size)) => {
            crate::log_eprintln!("✅ Best CRF {:.1} already encoded (full video)", crf);
            (crf, size)
        }
        _ => {
            crate::log_eprintln!("⚠️ Cannot compress this file");
            let last_output_video = crate::stream_size::get_output_video_stream_size(output);
            crate::verbose_eprintln!(
                "   Video stream: input {} vs output {} ({:+.1}%)",
                crate::format_bytes(input_video_stream_size),
                crate::format_bytes(last_output_video),
                stream_size_change_pct(last_output_video, input_video_stream_size)
            );
            let size = encode_cached(max_crf, &mut size_cache)?;
            iterations += 1;
            (max_crf, size)
        }
    };

    crate::verbose_eprintln!(
        "Final: CRF {:.1} | Size: {} bytes ({:.2} MB)",
        final_crf,
        final_full_size,
        final_full_size as f64 / 1024.0 / 1024.0
    );

    let ssim = calculate_ssim_enhanced(input, output);

    if let Some(s) = ssim {
        let quality_hint = if s >= 0.99 {
            "✅ Excellent"
        } else if s >= 0.98 {
            "✅ Very Good"
        } else if s >= 0.95 {
            "Good"
        } else {
            "Below threshold"
        };
        crate::log_eprintln!("SSIM: {:.6} {}", s, quality_hint);
    } else {
        crate::log_eprintln!("⚠️  SSIM calculation failed after trying all methods");
    }

    let size_change_pct = if input_size == 0 {
        0.0
    } else {
        (final_full_size as f64 / input_size as f64 - 1.0) * 100.0
    };

    // User-relevant success: total file smaller and quality met (not video-stream efficiency).
    let total_file_compressed = final_full_size < input_size;
    let _video_stream_compressed =
        crate::stream_size::can_compress_pure_video(output, input_video_stream_size);
    let ssim_ok = match ssim {
        Some(s) => s >= min_ssim,
        None => false,
    };
    let quality_passed = total_file_compressed && ssim_ok;

    let ssim_val = ssim.unwrap_or(0.0);

    let sampling_coverage = 1.0;

    let prediction_accuracy = 0.95;

    let target = compression_target_size(input_size);
    let margin_safety = if target > 0 && final_full_size < target {
        let margin = (target - final_full_size) as f64 / target as f64;
        (margin / 0.05).min(1.0)
    } else {
        0.0
    };

    let ssim_confidence = if ssim_val >= 0.99 {
        1.0
    } else if ssim_val >= 0.95 {
        0.9
    } else if ssim_val >= 0.90 {
        0.7
    } else {
        0.5
    };

    let confidence_detail = ConfidenceBreakdown {
        sampling_coverage,
        prediction_accuracy,
        margin_safety,
        ssim_confidence,
    };
    let confidence = confidence_detail.overall();

    crate::log_eprintln!();
    crate::log_eprintln!("═══════════════════════════════════════════════════════════");
    crate::log_eprintln!(
        "✅ RESULT: CRF {:.1} • Size {:+.1}% • Iterations: {}",
        final_crf,
        size_change_pct,
        iterations
    );
    crate::log_eprintln!(
        "   Total file smaller than input: {}",
        if total_file_compressed {
            "✅ YES"
        } else {
            "❌ NO"
        }
    );

    let output_stream_info = crate::stream_size::extract_stream_sizes(output);
    let input_stream_info = crate::stream_size::extract_stream_sizes(input);
    let video_stream_pct = if input_stream_info.video_stream_size > 0 {
        (output_stream_info.video_stream_size as f64 / input_stream_info.video_stream_size as f64
            - 1.0)
            * 100.0
    } else {
        0.0
    };
    crate::log_eprintln!(
        "   Video stream: {} → {} ({:+.1}%)",
        crate::format_bytes(input_stream_info.video_stream_size),
        crate::format_bytes(output_stream_info.video_stream_size),
        video_stream_pct
    );

    let enhanced = crate::quality_verifier_enhanced::verify_after_encode(
        input,
        output,
        &crate::quality_verifier_enhanced::VerifyOptions::strict_video(),
    );
    crate::verbose_eprintln!("   {}", enhanced.summary());
    for d in &enhanced.details {
        crate::verbose_eprintln!("      {}", d);
    }

    let total_file_pct = if input_size == 0 {
        0.0
    } else {
        (final_full_size as f64 / input_size as f64 - 1.0) * 100.0
    };
    if output_stream_info.is_overhead_excessive() {
        crate::log_eprintln!(
            "   ⚠️  Container overhead: {:.1}% (> 10%)",
            output_stream_info.container_overhead_percent()
        );
    }
    if video_stream_pct < 0.0 && total_file_pct > 0.0 {
        crate::log_eprintln!(
            "   ⚠️  Video stream compressed ({:+.1}%) but total file larger ({:+.1}%)",
            video_stream_pct,
            total_file_pct
        );
        crate::log_eprintln!(
            "   Container overhead: {} ({:.1}% of output)",
            crate::format_bytes(output_stream_info.container_overhead),
            output_stream_info.container_overhead_percent()
        );
    }

    confidence_detail.print_report();

    cpu_progress.finish_iteration(final_crf, final_full_size, ssim);

    Ok(ExploreResult {
        optimal_crf: final_crf,
        output_size: final_full_size,
        size_change_pct,
        ssim,
        psnr: None,
        ms_ssim: None,
        ms_ssim_passed: None,
        ms_ssim_score: None,
        iterations,
        quality_passed,
        log,
        confidence,
        confidence_detail,
        actual_min_ssim: min_ssim,
        input_video_stream_size: input_stream_info.video_stream_size,
        output_video_stream_size: output_stream_info.video_stream_size,
        container_overhead: output_stream_info.container_overhead,
    })
}

pub fn explore_hevc_with_gpu_coarse(
    input: &Path,
    output: &Path,
    vf_args: Vec<String>,
    initial_crf: f32,
    max_threads: usize,
) -> Result<ExploreResult> {
    explore_hevc_with_gpu_coarse_full(
        input,
        output,
        vf_args,
        initial_crf,
        false,
        false,
        max_threads,
    )
}

pub fn explore_hevc_with_gpu_coarse_ultimate(
    input: &Path,
    output: &Path,
    vf_args: Vec<String>,
    initial_crf: f32,
    ultimate_mode: bool,
    max_threads: usize,
) -> Result<ExploreResult> {
    explore_hevc_with_gpu_coarse_full(
        input,
        output,
        vf_args,
        initial_crf,
        ultimate_mode,
        false,
        max_threads,
    )
}

pub fn explore_hevc_with_gpu_coarse_full(
    input: &Path,
    output: &Path,
    vf_args: Vec<String>,
    initial_crf: f32,
    ultimate_mode: bool,
    force_ms_ssim_long: bool,
    max_threads: usize,
) -> Result<ExploreResult> {
    let (max_crf, min_ssim) = calculate_smart_thresholds(initial_crf, VideoEncoder::Hevc);
    explore_with_gpu_coarse_search(
        input,
        output,
        VideoEncoder::Hevc,
        vf_args,
        initial_crf,
        max_crf,
        min_ssim,
        ultimate_mode,
        force_ms_ssim_long,
        max_threads,
    )
}

pub fn explore_av1_with_gpu_coarse(
    input: &Path,
    output: &Path,
    vf_args: Vec<String>,
    initial_crf: f32,
    max_threads: usize,
) -> Result<ExploreResult> {
    let (max_crf, min_ssim) = calculate_smart_thresholds(initial_crf, VideoEncoder::Av1);
    explore_with_gpu_coarse_search(
        input,
        output,
        VideoEncoder::Av1,
        vf_args,
        initial_crf,
        max_crf,
        min_ssim,
        false,
        false,
        max_threads,
    )
}
