//! GPU coarse search and CPU fine-tuning for CRF exploration

use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

use super::calibration;
use super::dynamic_mapping;
use super::precheck;
use super::*;

/// Build the colour/HDR FFmpeg arguments from an FFprobeResult.
/// These arguments must be appended to every final HEVC/AV1/H.264 encode so that
/// colour metadata (primaries, TRC, matrix, mastering display, CLL) is preserved.
fn build_color_args_from_probe(probe: &crate::ffprobe::FFprobeResult) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();

    if let Some(ref cp) = probe.color_primaries {
        if !cp.is_empty() && cp != "unknown" {
            args.push("-color_primaries".to_string());
            args.push(cp.clone());
        }
    }
    if let Some(ref trc) = probe.color_transfer {
        if !trc.is_empty() && trc != "unknown" {
            args.push("-color_trc".to_string());
            args.push(trc.clone());
        }
    }
    if let Some(ref cs) = probe.color_space {
        // Normalise bt2020ncl/bt2020nc_l variants that ffprobe sometimes emits
        let normalised = match cs.as_str() {
            "bt2020ncl" | "bt2020_ncl" => "bt2020nc",
            "bt2020cl"  | "bt2020_cl"  => "bt2020c",
            other => other,
        };
        // Skip RGB/GBR colorspace: HEVC doesn't support it, and we're converting to YUV in filter chain
        let is_rgb_colorspace = normalised == "gbr" || normalised == "rgb" || normalised == "gbrp";
        if !normalised.is_empty() && normalised != "unknown" && !is_rgb_colorspace {
            args.push("-colorspace".to_string());
            args.push(normalised.to_string());
        }
    }
    if let Some(ref md) = probe.mastering_display {
        if !md.is_empty() {
            args.push("-master_display".to_string());
            args.push(md.clone());
        }
    }
    if let Some(ref cll) = probe.max_cll {
        if !cll.is_empty() {
            args.push("-max_cll".to_string());
            args.push(cll.clone());
        }
    }
    args
}

/// Return the correct pixel format for encoding: yuv420p10le for 10-bit HDR content,
/// yuv420p for 8-bit SDR. Preserving the bit depth is essential for HDR accuracy.
fn pick_pix_fmt(probe: &crate::ffprobe::FFprobeResult) -> &'static str {
    if probe.bit_depth >= 10 {
        "yuv420p10le"
    } else {
        "yuv420p"
    }
}

/// Percentage change from input stream size (avoids div-by-zero / inf when input is 0).
#[inline]
fn stream_size_change_pct(output_size: u64, input_size: u64) -> f64 {
    let denom = input_size.max(1) as f64;
    (output_size as f64 / denom - 1.0) * 100.0
}

/// Format the QualityCheck log line from result; used for logging and unit tests (regression: enhanced failure shows reason, not "total file not smaller").
pub(crate) fn format_quality_check_line(
    result: &ExploreResult,
    quality_verification_skipped_for_format: bool,
) -> String {
    match (result.ms_ssim_passed, result.quality_passed) {
        (Some(false), _) => {
            "   QualityCheck: FAILED (quality metrics below target)".to_string()
        }
        (Some(true), true) => "   QualityCheck: PASSED (quality + total file size target met)".to_string(),
        (Some(true), false) => {
            if let Some(ref reason) = result.enhanced_verify_fail_reason {
                format!(
                    "   QualityCheck: FAILED (quality met but enhanced verification failed: {})",
                    reason
                )
            } else {
                "   QualityCheck: FAILED (quality met but total file not smaller)".to_string()
            }
        }
        (None, true) => "   QualityCheck: PASSED (total file size target met)".to_string(),
        (None, false) if quality_verification_skipped_for_format => {
            "   QualityCheck: N/A (GIF/size-only, quality not measured)".to_string()
        }
        (None, false) => "   QualityCheck: FAILED (quality not verified)".to_string(),
    }
}

#[allow(clippy::too_many_arguments)]
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
    allow_size_tolerance: bool,
    max_threads: usize,
) -> Result<ExploreResult> {
    use crate::gpu_accel::{CrfMapping, GpuAccel, GpuCoarseConfig};

    let precheck_info = precheck::run_precheck(input)?;
    let _compressibility = precheck_info.compressibility;
    crate::log_eprintln!();

    let input_size = fs::metadata(input)
        .context("Failed to read input file metadata")?
        .len();

    let mut best_vmaf_tracked: Option<f64> = None;
    let mut best_psnr_uv_tracked: Option<(f64, f64)> = None;

    let gpu = GpuAccel::detect();
    gpu.print_detection_info();
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

    // Single ffprobe call — result is reused in Phase 3 and audio strategy detection.
    let probe_result = crate::ffprobe::probe_video(input).ok();
    let duration: f32 = probe_result
        .as_ref()
        .map(|p| p.duration as f32)
        .unwrap_or(crate::gpu_accel::GPU_SAMPLE_DURATION);

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

        let sample_dur = if ultimate_mode {
            crate::gpu_accel::GPU_SAMPLE_DURATION_ULTIMATE
        } else {
            crate::gpu_accel::GPU_SAMPLE_DURATION
        };
        let gpu_sample_input_size = if duration <= sample_dur {
            input_size
        } else {
            let ratio = sample_dur / duration;
            (input_size as f64 * ratio as f64) as u64
        };

        let gpu_config = GpuCoarseConfig {
            initial_crf,
            min_crf: 0.0,
            max_crf,
            step: 2.0,
            max_iterations: crate::gpu_accel::GPU_ABSOLUTE_MAX_ITERATIONS,
            ultimate_mode,
        };

        let gpu_progress = crate::UnifiedProgressBar::new_iteration(
            "[GPU] Coarse Search",
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

                    let dynamic_mapper = dynamic_mapping::quick_calibrate(
                        input,
                        input_size,
                        encoder,
                        &vf_args,
                        gpu_encoder_name,
                        sample_dur,
                    )
                    .unwrap_or_else(|_| dynamic_mapping::DynamicCrfMapper::new(input_size));

                    let mapping = match encoder {
                        VideoEncoder::Hevc => CrfMapping::hevc(gpu.gpu_type),
                        VideoEncoder::Av1 => CrfMapping::av1(gpu.gpu_type),
                        // H.264 CRF range matches HEVC (0–51); reuse HEVC mapping for CPU search range.
                        VideoEncoder::H264 => CrfMapping::hevc(gpu.gpu_type),
                    };

                    let max_crf = match encoder {
                        VideoEncoder::Av1 => 63.0,
                        VideoEncoder::Hevc | VideoEncoder::H264 => 51.0,
                    };
                    let (dynamic_cpu_crf, dynamic_confidence) = if dynamic_mapper.calibrated {
                        dynamic_mapper.print_calibration_report();
                        dynamic_mapper.gpu_to_cpu(gpu_crf, mapping.offset, max_crf)
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
                crate::log_eprintln!("⚠️  FALLBACK: GPU coarse search failed: {} (falling back to CPU-only)", e);
                (ABSOLUTE_MIN_CRF, max_crf, initial_crf)
            }
        }
    } else {
        crate::log_eprintln!();
        if !gpu.is_available() {
            crate::log_eprintln!("⚠️  FALLBACK: No GPU available (skipping GPU coarse phase)");
        } else {
            crate::log_eprintln!("⚠️  FALLBACK: No GPU encoder for {:?} (skipping GPU coarse phase)", encoder);
        }
        (ABSOLUTE_MIN_CRF, max_crf, initial_crf)
    };

    crate::verbose_eprintln!("Phase 2: 🖥️  CPU Fine-Tune (0.5→0.1 step)");
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
        allow_size_tolerance,
        max_threads,
        duration,
        probe_result.as_ref(),
        &mut best_vmaf_tracked,
        &mut best_psnr_uv_tracked,
    )?;

    result.log.clear();

    // Skip quality verification if early insight triggered
    if result.early_insight_triggered {
        crate::log_eprintln!();
        crate::log_eprintln!("{}═══════════════════════════════════════════════════════════{}",
            crate::modern_ui::colors::BRIGHT_YELLOW, crate::modern_ui::colors::RESET);
        crate::log_eprintln!("{}⚠️  Early Insight Triggered: Quality Plateau Detected{}",
            crate::modern_ui::colors::BRIGHT_YELLOW, crate::modern_ui::colors::RESET);
        crate::log_eprintln!("   No integer-level quality improvement over 3 consecutive iterations");
        crate::log_eprintln!("   Skipping final quality verification (unnecessary)");
        crate::log_eprintln!("{}═══════════════════════════════════════════════════════════{}",
            crate::modern_ui::colors::BRIGHT_YELLOW, crate::modern_ui::colors::RESET);
        return Ok(result);
    }

    crate::verbose_eprintln!();
    crate::verbose_eprintln!("Phase 3: Quality Verification");

    let mut quality_verification_skipped_for_format = false;

    if let Some(probe_result) = probe_result.as_ref() {
        let duration = probe_result.duration;
        crate::verbose_eprintln!(
            "   Video duration: {:.1}s ({:.1} min)",
            duration,
            duration / 60.0
        );

        /// Normal mode: skip MS-SSIM for videos longer than 5 min (cost/quality tradeoff).
        const VMAF_DURATION_THRESHOLD_SECS: f64 = 300.0;
        /// Ultimate mode: allow MS-SSIM up to 25 min for stricter quality verification.
        const VMAF_DURATION_THRESHOLD_ULTIMATE_SECS: f64 = 1500.0;

        let ms_ssim_duration_threshold_secs = if ultimate_mode {
            VMAF_DURATION_THRESHOLD_ULTIMATE_SECS
        } else {
            VMAF_DURATION_THRESHOLD_SECS
        };
        let is_gif_format = probe_result.format_name.eq_ignore_ascii_case("gif");

        let should_run_vmaf =
            !is_gif_format && (duration <= ms_ssim_duration_threshold_secs || force_ms_ssim_long);

        if is_gif_format {
            crate::verbose_eprintln!(
                "   GIF input: using SSIM-All verification (ffmpeg ssim filter, GIF-compatible)"
            );

            if let Some((y, u, v, all)) = calculate_ssim_all(input, output) {
                crate::log_eprintln!("   SSIM Y/U/V/All: {:.4}/{:.4}/{:.4}/{:.4}", y, u, v, all);
                let gif_threshold = result.actual_min_ssim.max(0.92);
                if all < gif_threshold {
                    crate::log_eprintln!(
                        "   ❌ SSIM ALL BELOW TARGET! {:.4} < {:.2}",
                        all,
                        gif_threshold
                    );
                    result.ms_ssim_passed = Some(false);
                } else {
                    crate::log_eprintln!(
                        "   ✅ SSIM ALL TARGET MET: {:.4} ≥ {:.2}",
                        all,
                        gif_threshold
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
            let threshold_min = ms_ssim_duration_threshold_secs / 60.0;
            crate::log_eprintln!(
                "   Video within limit (≤{:.0}min)",
                threshold_min
            );

            if ultimate_mode {
                // ── Ultimate Mode: 3D Quality Gate ────────────────────────────
                // Three independent dimensions must ALL pass:
                //   1. VMAF-Y   ≥ 93.0   (perceptual quality, Netflix standard)
                //   2. CAMBI    ≤ 5.0    (banding detection, lower = better, Netflix standard)
                //   3. PSNR-UV  ≥ 38.0 dB (chroma fidelity)
                crate::log_eprintln!("   Enabling precision quality gate (Ultimate Mode)...");

                // Determine sample rate from duration (mirrors calculate_ms_ssim_yuv logic)
                let duration_min = probe_result.duration / 60.0;
                let sample_rate: usize = if duration_min <= 1.0 { 1 } else { 3 };

                // Reuse metrics from search phase if available, otherwise calculate
                let vmaf_y = if let Some(v) = best_vmaf_tracked {
                    crate::verbose_eprintln!("      ℹ️  Reusing VMAF from search phase: {:.2}", v);
                    Some(v)
                } else {
                    super::ssim_calculator::calculate_vmaf_y(input, output, sample_rate)
                };

                let psnr_uv = if let Some(uv) = best_psnr_uv_tracked {
                    crate::verbose_eprintln!("      ℹ️  Reusing PSNR-UV from search phase: {:.2}/{:.2}", uv.0, uv.1);
                    Some(uv)
                } else {
                    super::ssim_calculator::calculate_psnr_uv(input, output, sample_rate)
                };

                // CAMBI is only measured in Phase III as the final banding check
                crate::log_eprintln!("   Running final CAMBI banding check...");
                let cambi = super::ssim_calculator::calculate_cambi(output, sample_rate);

                // Thresholds
                const VMAF_Y_THRESHOLD: f64 = 93.0;
                const CAMBI_MAX: f64        = 5.0;
                const PSNR_UV_MIN: f64      = 38.0;

                let vmaf_ok   = vmaf_y.map(|v| v >= VMAF_Y_THRESHOLD).unwrap_or(false);
                let cambi_ok  = cambi.map(|c| c <= CAMBI_MAX).unwrap_or(false);
                let chroma_ok = psnr_uv.map(|(u, v): (f64, f64)| u.min(v) >= PSNR_UV_MIN).unwrap_or(false);

                crate::log_eprintln!("   ═══════════════════════════════════════════════════");
                crate::log_eprintln!("   Quality Verification (Ultimate Mode):");

                match vmaf_y {
                    Some(v) => crate::log_eprintln!(
                        "      VMAF-Y: {:6.2} ≥ {:.1} {}",
                        v, VMAF_Y_THRESHOLD,
                        if vmaf_ok { "✅" } else { "❌" }
                    ),
                    None => crate::log_eprintln!(
                        "      VMAF-Y: N/A (calculation failed) ❌"
                    ),
                }

                match cambi {
                    Some(c) => crate::log_eprintln!(
                        "      CAMBI:  {:6.2} ≤ {:.1} {} (lower=better)",
                        c, CAMBI_MAX,
                        if cambi_ok { "✅" } else { "❌" }
                    ),
                    None => crate::log_eprintln!(
                        "      CAMBI: N/A (calculation failed) ❌"
                    ),
                }

                match psnr_uv {
                    Some((pu, pv)) => crate::log_eprintln!(
                        "      PSNR-UV: {:.2}/{:.2} dB ≥ {:.1} dB {}",
                        pu, pv, PSNR_UV_MIN,
                        if chroma_ok { "✅" } else { "❌" }
                    ),
                    None => crate::log_eprintln!(
                        "      PSNR-UV: N/A (calculation failed) ❌"
                    ),
                }

                crate::log_eprintln!("   ───────────────────────────────────────────────────");

                let all_passed = vmaf_ok && cambi_ok && chroma_ok;

                if all_passed {
                    crate::log_eprintln!("   ✅ QUALITY GATE: PASSED");
                    result.ms_ssim_passed = Some(true);
                    // Store a representative score (VMAF-Y) for log/report
                    result.ms_ssim_score = vmaf_y.map(|v| v / 100.0);
                    result.vmaf_y_score  = vmaf_y;
                    result.cambi_score   = cambi;
                    result.psnr_uv_score = psnr_uv;
                } else {
                    crate::log_eprintln!("   ❌ QUALITY GATE: FAILED");
                    if !vmaf_ok {
                        let v_str = vmaf_y.map(|v| format!("{:.2}", v)).unwrap_or_else(|| "N/A".to_string());
                        crate::log_eprintln!("      FAILED VMAF-Y {} < {:.1} (perceptual quality too low)", v_str, VMAF_Y_THRESHOLD);
                    }
                    if !cambi_ok {
                        let c_str = cambi.map(|c| format!("{:.2}", c)).unwrap_or_else(|| "N/A".to_string());
                        crate::log_eprintln!("      FAILED CAMBI {} > {:.1} (banding detected)", c_str, CAMBI_MAX);
                    }
                    if !chroma_ok {
                        let uv_str = psnr_uv
                            .map(|(u, v): (f64, f64)| format!("min={:.2}", u.min(v)))
                            .unwrap_or_else(|| "N/A".to_string());
                        crate::log_eprintln!("      FAILED PSNR-UV {} dB < {:.1} dB (chroma quality too low)", uv_str, PSNR_UV_MIN);
                    }
                    crate::log_eprintln!("      Suggestion: Lower CRF or disable --compress");
                    result.ms_ssim_passed = Some(false);
                    result.ms_ssim_score = vmaf_y.map(|v| v / 100.0);
                    result.vmaf_y_score  = vmaf_y;
                    result.cambi_score   = cambi;
                    result.psnr_uv_score = psnr_uv;
                }
            } else {
                // ── Normal Mode: Fusion (MS-SSIM + SSIM-All) ─────────────────
                crate::log_eprintln!("   Enabling fusion quality verification (MS-SSIM + SSIM)...");

                let max_duration_min = ms_ssim_duration_threshold_secs / 60.0;
                let ms_ssim_yuv_result = calculate_ms_ssim_yuv(input, output, max_duration_min);
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
                    crate::log_eprintln!("      MS-SSIM Y/U/V/Avg: {:.4}/{:.4}/{:.4} / {:.4}", y, u, v, avg);
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
            }
        } else {
            crate::log_eprintln!(
                "   ⚠️  Quality verification: long video (>{:.0}min), MS-SSIM skipped.",
                ms_ssim_duration_threshold_secs / 60.0
            );
            crate::log_eprintln!("   Using SSIM-All verification only.");

            if let Some((y, u, v, all)) = calculate_ssim_all(input, output) {
                crate::log_eprintln!("   SSIM Y/U/V/All: {:.4}/{:.4}/{:.4}/{:.4}", y, u, v, all);

                let long_threshold = result.actual_min_ssim.max(0.92);
                if all < long_threshold {
                    crate::log_eprintln!(
                        "   ❌ SSIM ALL BELOW TARGET! {:.4} < {:.2}",
                        all,
                        long_threshold
                    );
                    result.ms_ssim_passed = Some(false);
                } else {
                    crate::log_eprintln!(
                        "   ✅ SSIM ALL TARGET MET: {:.4} ≥ {:.2}",
                        all,
                        long_threshold
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

            let no_duration_threshold = result.actual_min_ssim.max(0.92);
            if all < no_duration_threshold {
                crate::log_eprintln!(
                    "   ❌ SSIM ALL BELOW TARGET! {:.4} < {:.2}",
                    all,
                    no_duration_threshold
                );
                result.ms_ssim_passed = Some(false);
            } else {
                crate::log_eprintln!(
                    "   ✅ SSIM ALL TARGET MET: {:.4} ≥ {:.2}",
                    all,
                    no_duration_threshold
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

    let quality_check_line =
        format_quality_check_line(&result, quality_verification_skipped_for_format);
    result.log.push(quality_check_line);

    crate::log_eprintln!();

    if gpu.is_available() && has_gpu_encoder {
        let mapping = match encoder {
            VideoEncoder::Hevc => CrfMapping::hevc(gpu.gpu_type),
            VideoEncoder::Av1 => CrfMapping::av1(gpu.gpu_type),
            VideoEncoder::H264 => CrfMapping::hevc(gpu.gpu_type), // same as above: H.264 reuses HEVC mapping
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

fn is_image_container(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    matches!(
        ext.as_str(),
        "avif" | "heic" | "heif" | "gif" | "webp" | "png" | "jpg" | "jpeg" | "bmp" | "tiff"
    )
}

#[allow(unused_assignments)]
#[allow(clippy::too_many_arguments)]
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
    allow_size_tolerance: bool,
    max_threads: usize,
    duration: f32,
    probe_info: Option<&crate::ffprobe::FFprobeResult>,
    best_vmaf_tracked: &mut Option<f64>,
    best_psnr_uv_tracked: &mut Option<(f64, f64)>,
) -> Result<ExploreResult> {
    #[allow(unused_mut)]
    let mut log = Vec::new();
    let mut early_insight_triggered = false;

    let input_size = fs::metadata(input)
        .context("Failed to read input file metadata")?
        .len();

    // Image containers (AVIF, HEIC, GIF, WebP, …) have no audio streams.
    // Mapping all streams (-map 0) causes FFmpeg libx265 to fail with
    // "Not yet implemented in FFmpeg, patches welcome".
    let input_is_image = is_image_container(input);

    let input_stream_info = crate::stream_size::extract_stream_sizes(input);
    let input_video_stream_size = input_stream_info.video_stream_size;
    crate::verbose_eprintln!(
        "{}Input video stream: {} (total file: {}, overhead: {:.1}%)",
        CYAN,
        crate::modern_ui::format_size(input_video_stream_size),
        crate::modern_ui::format_size(input_size),
        input_stream_info.container_overhead_percent()
    );

    let estimated_iterations = if ultimate_mode {
        let crf_range = max_crf - min_crf;
        let adaptive_walls = calculate_adaptive_max_walls(crf_range);
        (adaptive_walls + 10) as u64
    } else {
        15
    };
    let cpu_progress =
        crate::UnifiedProgressBar::new_iteration("[CPU] Fine-Tune", input_size, estimated_iterations);

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
            let audio_codec = probe_info
                .and_then(|info| info.audio_codec.as_ref())
                .map(|s| s.to_lowercase())
                .unwrap_or_default();
            let audio_bitrate = probe_info
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
            .arg(crate::safe_path_arg(input).as_ref());

        // Map streams: for image containers (AVIF/HEIC/GIF/WebP), only map video
        // to avoid FFmpeg libx265 "Not yet implemented" error when handling
        // non-existent audio streams.
        if input_is_image {
            cmd.arg("-map").arg("0:v");
        } else {
            cmd.arg("-map").arg("0");
        }

        cmd.arg("-c:v")
            .arg(encoder.ffmpeg_name())
            .arg("-crf")
            .arg(format!("{:.2}", crf));

        for arg in encoder.extra_args(max_threads) {
            cmd.arg(arg);
        }

        // Preserve pixel format (critical for 10-bit HDR content)
        if let Some(probe) = probe_info {
            let pix_fmt = pick_pix_fmt(probe);
            cmd.arg("-pix_fmt").arg(pix_fmt);

            // Forward all HDR colour metadata (primaries, TRC, colorspace, mastering display, CLL)
            for arg in build_color_args_from_probe(probe) {
                cmd.arg(arg);
            }
        }

        for arg in &vf_args {
            if !arg.is_empty() {
                cmd.arg(arg);
            }
        }

        if input_is_image {
             cmd.arg("-an");
        } else {
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
        }

        // Subtitle passthrough
        if let Some(probe) = probe_info {
            if probe.has_subtitles {
                let out_ext = output
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                let container = if out_ext == "mkv" { "mkv" } else { "mp4" };
                let sub_args = crate::subtitle_args_for_container(
                    true,
                    probe.subtitle_codec.as_deref(),
                    container,
                );
                for arg in sub_args {
                    cmd.arg(arg);
                }
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
        "{}Using 0.25 step (upward) + 0.1 step (downward, aligned with main path){}",
        CYAN,
        RESET
    );
    let step_size_upward = 0.25_f32;
    const PHASE3_DOWNWARD_STEP: f32 = 0.1;

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
        step_size_upward,
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

    let gpu_size = encode_cached(gpu_boundary_crf, &mut size_cache).map_err(|e| {
        crate::log_eprintln!(
            "{}⚠️  Boundary verification failed at CRF {:.1}{}",
            BRIGHT_YELLOW,
            gpu_boundary_crf,
            RESET
        );
        crate::log_eprintln!("   Error: {}", e);
        e
    })?;
    iterations += 1;
    let gpu_pct = if input_size > 0 {
        (gpu_size as f64 / input_size as f64 - 1.0) * 100.0
    } else {
        0.0
    };
    let gpu_ssim = calculate_ssim_quick();

    let is_gpu_effectively_compressed = gpu_size < input_size;

    if is_gpu_effectively_compressed {
        best_crf = Some(gpu_boundary_crf);
        best_size = Some(gpu_size);
        best_ssim_tracked = gpu_ssim;
        
        let mut gpu_ultimate_metrics_str = String::new();
        if ultimate_mode {
            let vmaf = super::ssim_calculator::calculate_vmaf_y(input, output, 6);
            let psnr_uv = super::ssim_calculator::calculate_psnr_uv(input, output, 6);
            if let (Some(v), Some((u, v_score))) = (vmaf, psnr_uv) {
                let chroma_avg = (u + v_score) / 2.0;
                gpu_ultimate_metrics_str = format!("VMAF:{:.2} UV:{:.2}", v, chroma_avg);
                *best_vmaf_tracked = Some(v);
                *best_psnr_uv_tracked = Some((u, v_score));
            }
        }

        let metrics_display = if ultimate_mode && !gpu_ultimate_metrics_str.is_empty() {
            format!(" │ {}", gpu_ultimate_metrics_str)
        } else if let Some(s) = gpu_ssim {
            format!(" │ SSIM:{:.4}", s)
        } else {
            " │ SSIM N/A".to_string()
        };

        use crate::modern_ui::colors::*;
        crate::log_eprintln!(
            "{}{}   {}✓{} [GPU] {}CRF {:<4.1}{} {}{:6.1}%{}{} ✅",
            RESET, RESET, BRIGHT_GREEN, RESET, CYAN, gpu_boundary_crf, RESET,
            BRIGHT_GREEN, gpu_pct, RESET, metrics_display
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

        let search_floor = if ultimate_mode { 0.0 } else { min_crf };
        let crf_range = gpu_boundary_crf - search_floor;

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

        let max_iterations_for_video = if ultimate_mode { 200 } else { calculate_max_iterations_for_duration(duration, ultimate_mode) };

        if ultimate_mode {
            crate::verbose_eprintln!(
                "   {}ULTIMATE MODE: searching until SSIM saturation (Domain Wall){}",
                BRIGHT_MAGENTA,
                RESET
            );
            crate::verbose_eprintln!("   {}CRF range: {:.1} → Adaptive max walls: {}{}{}{}",
                DIM, crf_range, BRIGHT_CYAN, max_wall_hits, RESET, RESET);
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

        let gpu_ssim_baseline = match gpu_ssim {
            Some(s) => s,
            None => {
                crate::log_eprintln!("   {}⚠️  GPU SSIM not measured, cannot establish baseline{}", BRIGHT_YELLOW, RESET);
                bail!("GPU SSIM baseline not available");
            }
        };
        crate::verbose_eprintln!(
            "   {}GPU SSIM baseline: {}{:.4}{} (CPU target: break through 0.97+)",
            DIM,
            BRIGHT_YELLOW,
            gpu_ssim_baseline,
            RESET
        );

        const ZERO_GAIN_THRESHOLD: f64 = 0.00005;

        let mut consecutive_zero_gains: u32 = 0;
        let mut failure_credibility: f64 = 0.0;
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

        // Determine search floor based on mode
        // Ultimate Mode has NO floor (0.0) to allow hitting the true physical wall.
        let search_floor = if ultimate_mode { 0.0 } else { min_crf };

        while iterations < max_iterations_for_video && test_crf >= search_floor {
            if test_crf < search_floor {
                if current_step > MIN_STEP + 0.01 {
                    crate::verbose_eprintln!(
                        "   {}Reached search floor, fine tuning from CRF {:.1}{}",
                        BRIGHT_CYAN,
                        last_good_crf,
                        RESET
                    );
                    current_step = MIN_STEP;
                    test_crf = last_good_crf - current_step;
                    if test_crf < search_floor {
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
            let total_size_pct = if input_size > 0 {
                (size as f64 / input_size as f64 - 1.0) * 100.0
            } else {
                0.0
            };
            let current_ssim_opt = calculate_ssim_quick();

            let is_effectively_compressed = size < input_size;

            if is_effectively_compressed {
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
                        let _gpu_comparison = if ssim_vs_gpu > 1.01 {
                            format!("{}×{:.3} GPU{}", BRIGHT_GREEN, ssim_vs_gpu, RESET)
                        } else if ssim_vs_gpu > 1.001 {
                            format!("{}×{:.4} GPU{}", GREEN, ssim_vs_gpu, RESET)
                        } else {
                            format!("{}≈GPU{}", DIM, RESET)
                        };

                        let is_zero_gain = ssim_gain.abs() < ZERO_GAIN_THRESHOLD;
                        
                        // Ultimate Mode: Unified Multi-Metric Tracking
                        let mut ultimate_metrics_str = String::new();
                        let mut quality_saturated = false;
                        
                        if ultimate_mode {
                            let vmaf = super::ssim_calculator::calculate_vmaf_y(input, output, 6);
                            let psnr_uv = super::ssim_calculator::calculate_psnr_uv(input, output, 6);

                            if let (Some(v), Some((u, v_score))) = (vmaf, psnr_uv) {
                                let chroma_avg = (u + v_score) / 2.0;

                                // Check for integer-level improvement
                                let prev_best_vmaf = best_vmaf_tracked.unwrap_or(0.0);
                                let prev_best_psnr = best_psnr_uv_tracked.map(|(u,v)| (u+v)/2.0).unwrap_or(0.0);
                                let vmaf_improved = v.floor() > prev_best_vmaf.floor();
                                let psnr_improved = chroma_avg.floor() > prev_best_psnr.floor();

                                ultimate_metrics_str = format!("VMAF:{:.2} UV:{:.2}", v, chroma_avg);

                                // Update best tracked values
                                if vmaf_improved || best_vmaf_tracked.is_none() { *best_vmaf_tracked = Some(v); }
                                if psnr_improved || best_psnr_uv_tracked.is_none() { *best_psnr_uv_tracked = Some((u, v_score)); }

                                // Early insight: only trigger if quality fails threshold AND no improvement
                                const VMAF_Y_MIN: f64 = 93.0;
                                const PSNR_UV_MIN: f64 = 38.0;
                                let any_metric_fails = v < VMAF_Y_MIN || u.min(v_score) < PSNR_UV_MIN;

                                if !vmaf_improved && !psnr_improved && any_metric_fails {
                                    failure_credibility += 1.0;
                                    if failure_credibility >= 3.0 {
                                        crate::log_eprintln!(
                                            "   {}❌ QUALITY PLATEAU REACHED (3/3):{} No integer improvement over 3 insights. Stopping.",
                                            BRIGHT_RED, RESET
                                        );
                                        early_insight_triggered = true;
                                        break;
                                    }
                                } else {
                                    failure_credibility = 0.0;
                                }

                                // Saturation Check: In high quality zones (>97), we monitor for gain stop
                                if v > 97.0 || chroma_avg > 47.0 {
                                    quality_saturated = true;
                                }
                            }
                        }

                        if current_step <= MIN_STEP + 0.01 {
                            // Unified saturation counter: SSIM flat OR Quality high and flat
                            if is_zero_gain || quality_saturated {
                                consecutive_zero_gains += 1;
                            } else {
                                consecutive_zero_gains = 0;
                            }
                        }

                        // THE RED LINE: Hit the wall when either:
                        // 1. We reached 30 consecutive zero gains (Physical Saturation)
                        // 2. We reached required_zero_gains (Normal mode)
                        let quality_wall_triggered = current_step <= MIN_STEP + 0.01 && 
                            consecutive_zero_gains >= required_zero_gains;

                        // HIGH CONFIDENCE GATE: If we hit the wall but quality is still garbage, 
                        // this encode is not credible. Abort immediately.
                        if ultimate_mode && quality_wall_triggered {
                            const VMAF_Y_MIN: f64 = 93.0;
                            const PSNR_UV_MIN: f64 = 38.0;

                            let v_val = match best_vmaf_tracked {
                                Some(v) => v,
                                None => {
                                    crate::log_eprintln!("   {}⚠️  VMAF not measured at quality wall{}", BRIGHT_YELLOW, RESET);
                                    bail!("Quality wall hit but VMAF not measured");
                                }
                            };
                            let uv_val = match best_psnr_uv_tracked {
                                Some((u, v)) => (*u).min(*v),
                                None => {
                                    crate::log_eprintln!("   {}⚠️  PSNR UV not measured at quality wall{}", BRIGHT_YELLOW, RESET);
                                    bail!("Quality wall hit but PSNR UV not measured");
                                }
                            };

                            if *v_val < VMAF_Y_MIN || uv_val < PSNR_UV_MIN {
                                crate::log_eprintln!(
                                    "   \x1b[1;31m❌ QUALITY CEILING HIT (NOT CREDIBLE):\x1b[0m Saturated at VMAF:{:.2}, UV:{:.2}. Below mandatory gate. Aborting.",
                                    v_val, uv_val
                                );
                                quality_wall_hit = true;
                                break; 
                            }
                        }

                        let sat_status = if consecutive_zero_gains > 0 && current_step <= MIN_STEP + 0.01 {
                            format!(" {}[SAT:{}/{}]{}", 
                                if ultimate_mode { BRIGHT_MAGENTA } else { DIM },
                                consecutive_zero_gains, 
                                required_zero_gains, 
                                RESET
                            )
                        } else {
                            String::new()
                        };

                        let metrics_display = if ultimate_mode && !ultimate_metrics_str.is_empty() {
                            format!("{}{}{}", BRIGHT_MAGENTA, ultimate_metrics_str, RESET)
                        } else {
                            format!(" │ SSIM:{:.4} Δ{:+.4}", current_ssim, ssim_gain)
                        };

                        use crate::modern_ui::colors::*;
                        crate::log_eprintln!("{}{}   {}✓{} [CPU] {}CRF {:<4.1}{} {}{:6.1}% {} {}{}",
                            RESET, RESET, BRIGHT_GREEN, RESET, CYAN, test_crf, RESET,
                            MFB_BLUE, total_size_pct, RESET, metrics_display, sat_status);

                        if quality_wall_triggered {
                            quality_wall_hit = true;
                        }
                        quality_wall_triggered
                    }
                    _ => {
                        use crate::modern_ui::colors::*;
                        crate::log_eprintln!("{}{}   {}✓{} [CPU] {}CRF {:<4.1}{} {}{:6.1}% {} │ SSIM N/A",
                            RESET, RESET, BRIGHT_GREEN, RESET, CYAN, test_crf, RESET,
                            MFB_BLUE, total_size_pct, RESET);
                        false
                    }
                };

                if should_stop {
                    crate::log_eprintln!();
                    if ultimate_mode {
                        domain_wall_hit = true;
                        let msg = if consecutive_zero_gains >= required_zero_gains {
                            format!("SSIM saturated after {} consecutive zero-gains", consecutive_zero_gains)
                        } else {
                            "VMAF(Y) + PSNR(UV) absolute quality ceiling reached".to_string()
                        };
                        crate::log_eprintln!("   {} [CPU] DOMAIN WALL HIT:{} {}",
                            BRIGHT_MAGENTA, RESET, msg);
                    } else {
                        crate::log_eprintln!("   {} [CPU] QUALITY WALL HIT:{} SSIM saturated after {} consecutive zero-gains",
                            BRIGHT_YELLOW, RESET, consecutive_zero_gains);
                    }
                    crate::verbose_eprintln!(
                        "   {}Final: CRF {:.1}, compression {:+.1}%, iterations {}{}",
                        BRIGHT_CYAN,
                        test_crf,
                        total_size_pct,
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

                let _total_file_diff = crate::format_size_diff(size as i64 - input_size as i64);
                
                // Calculate new_step first for phase_info
                let curve_step = initial_step * DECAY_FACTOR.powi(wall_hits as i32);
                let new_step = if curve_step < 1.0 {
                    MIN_STEP
                } else {
                    curve_step
                };
                
                let phase_info = if wall_hits == 1 {
                    format!("decay ×{:.1}", DECAY_FACTOR)
                } else if new_step <= MIN_STEP + 0.01 {
                    "→ FINE TUNING".to_string()
                } else {
                    format!("decay {}×{:.1}^{}", DIM, DECAY_FACTOR, wall_hits)
                };

                use crate::modern_ui::colors::*;
                crate::log_eprintln!(
                    "{}{}   {}✗{} [CPU] {}CRF {:<4.1}{} {}{:6.1}% {} │ ❌ WALL HIT #{} (Backtrack: {:.2} → {:.2} {})",
                    RESET, RESET, BRIGHT_RED, RESET, CYAN, test_crf, RESET,
                    DIM, total_size_pct, RESET, wall_hits, current_step, new_step, phase_info
                );

                if current_step <= MIN_STEP + 0.01 && new_step <= MIN_STEP + 0.01 {
                    crate::log_eprintln!(
                        "   {} [CPU] 🧱 Minimum step reached and hit wall again. Stopping.{}",
                        BRIGHT_YELLOW,
                        RESET
                    );
                    break;
                }

                if wall_hits >= max_wall_hits {
                    if ultimate_mode {
                        crate::log_eprintln!(
                            "   {} [CPU] Adaptive wall limit ({}) reached.{} Stopping at best CRF {:.1}",
                            BRIGHT_YELLOW,
                            max_wall_hits,
                            RESET,
                            last_good_crf
                        );
                    } else {
                        crate::log_eprintln!(
                            "   {} [CPU] Max wall hits ({}) reached.{} Stopping at best CRF {:.1}",
                            BRIGHT_YELLOW,
                            max_wall_hits,
                            RESET,
                            last_good_crf
                        );
                    }
                    break;
                }

                current_step = new_step;
                test_crf = last_good_crf - current_step;
            }
        }

        if domain_wall_hit || quality_wall_hit {
            if best_crf.is_none_or(|c| c > last_good_crf) {
                best_crf = Some(last_good_crf);
                best_size = Some(last_good_size);
                best_ssim_tracked = last_good_ssim;
            }
        } else if overshoot_detected {
            crate::log_eprintln!();
            crate::log_eprintln!(
                "   {} [CPU] Size wall hit: overshoot at CRF < {:.1}{}",
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
        } else if test_crf < search_floor {
            crate::log_eprintln!();
            crate::log_eprintln!(
                "   {} [CPU] Search floor reached ({:.1}) - maximum quality achieved{}",
                BRIGHT_GREEN,
                search_floor,
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
        use crate::modern_ui::colors::*;
        crate::log_eprintln!(
            "{}{}   {}✗{} [CPU] {}CRF {:<4.1}{} {}{:6.1}%{} ❌ (TOO LARGE)",
            RESET, RESET, BRIGHT_RED, RESET, CYAN, gpu_boundary_crf, RESET,
            BRIGHT_RED, gpu_pct, RESET
        );
        crate::log_eprintln!();
        crate::log_eprintln!("Phase 2: [CPU] Search UPWARD for compression boundary");
        crate::log_eprintln!("   (Higher CRF = Smaller file, find first compressible)");

        let mut test_crf = gpu_boundary_crf + step_size_upward;
        let mut found_compress_point = false;
        let mut failure_credibility = 0.0f64;
        let mut best_tested_crf = gpu_boundary_crf;
        let mut best_tested_size = gpu_size;

        let max_iterations_for_video = if ultimate_mode { 150 } else { calculate_max_iterations_for_duration(duration, ultimate_mode) };

        while test_crf <= max_crf && iterations < max_iterations_for_video {
            let size = encode_cached(test_crf, &mut size_cache)?;
            iterations += 1;
            let total_size_pct = if input_size > 0 {
                (size as f64 / input_size as f64 - 1.0) * 100.0
            } else {
                0.0
            };

            // Ultimate Mode: Insight-Based Credibility Check (Sticky)
            if ultimate_mode {
                let vmaf = super::ssim_calculator::calculate_vmaf_y(input, output, 6);
                let psnr_uv = super::ssim_calculator::calculate_psnr_uv(input, output, 6);
                
                if let (Some(v), Some((u, v_score))) = (vmaf, psnr_uv) {
                    let chroma_avg = (u + v_score) / 2.0;
                    
                    // Track best metrics to check for improvement
                    let prev_best_vmaf = best_vmaf_tracked.unwrap_or(0.0);
                    let prev_best_psnr = best_psnr_uv_tracked.map(|(u,v)| (u+v)/2.0).unwrap_or(0.0);
                    
                    // Check for integer-level improvement (ignoring decimals)
                    let vmaf_improved = v.floor() > prev_best_vmaf.floor();
                    let psnr_improved = chroma_avg.floor() > prev_best_psnr.floor();
                    let improvement_indicator = if vmaf_improved || psnr_improved { "↑" } else { "→" };
                    use crate::modern_ui::colors::*;
                    crate::log_eprintln!(
                        "{}{}   {}✗{} [CPU] {}CRF {:<4.1}{} {}{:6.1}% {} │ VMAF:{:.2} UV:{:.2} ({:.1}/3.0 {})",
                        RESET, RESET, BRIGHT_RED, RESET, CYAN, test_crf, RESET,
                        DIM, total_size_pct, RESET, v, chroma_avg, failure_credibility, improvement_indicator
                    );

                    // Cache for Phase III and tracking
                    if vmaf_improved || best_vmaf_tracked.is_none() { *best_vmaf_tracked = Some(v); }
                    if psnr_improved || best_psnr_uv_tracked.is_none() { *best_psnr_uv_tracked = Some((u, v_score)); }

                    // Early insight: only trigger if quality fails threshold AND no improvement
                    const VMAF_Y_MIN: f64 = 93.0;
                    const PSNR_UV_MIN: f64 = 38.0;
                    let any_metric_fails = v < VMAF_Y_MIN || u.min(v_score) < PSNR_UV_MIN;

                    if !vmaf_improved && !psnr_improved && any_metric_fails {
                        failure_credibility += 1.0;
                        if failure_credibility >= 3.0 {
                            crate::log_eprintln!(
                                "   {}❌ QUALITY PLATEAU REACHED (3/3):{} No integer improvement over 3 insights. Stopping.",
                                BRIGHT_RED, RESET
                            );
                            // Use best tested CRF if no compression achieved
                            if best_crf.is_none() {
                                if size < input_size {
                                    best_crf = Some(test_crf);
                                    best_size = Some(size);
                                    best_ssim_tracked = calculate_ssim_quick();
                                } else {
                                    // No compression achieved - use best tested CRF
                                    best_crf = Some(best_tested_crf);
                                    best_size = Some(best_tested_size);
                                    best_ssim_tracked = calculate_ssim_quick();
                                }
                            }
                            early_insight_triggered = true;
                            break;
                        }
                    } else {
                        failure_credibility = 0.0;
                    }
                }
            }

            // Track best tested CRF (smallest size increase, even if not compressed)
            if size < best_tested_size {
                best_tested_crf = test_crf;
                best_tested_size = size;
            }

            let is_effectively_compressed = size < input_size;

            if is_effectively_compressed {
                if best_crf.is_none_or(|c| test_crf < c) {
                    best_crf = Some(test_crf);
                    best_size = Some(size);
                    best_ssim_tracked = calculate_ssim_quick();
                }
                found_compress_point = true;
                use crate::modern_ui::colors::*;
                crate::log_eprintln!(
                    "{}{}   {}✓{} [CPU] {}CRF {:<4.1}{} {}{:6.1}%{} │ FOUND! ✅",
                    RESET, RESET, BRIGHT_GREEN, RESET, CYAN, test_crf, RESET,
                    BRIGHT_GREEN, total_size_pct, RESET
                );
            } else {
                use crate::modern_ui::colors::*;
                crate::log_eprintln!(
                    "{}{}   {}✗{} [CPU] {}CRF {:<4.1}{} {}{:6.1}%{} ❌",
                    RESET, RESET, BRIGHT_RED, RESET, CYAN, test_crf, RESET,
                    BRIGHT_RED, total_size_pct, RESET
                );
            }
            test_crf += step_size_upward;
        }

        if !found_compress_point {
            crate::log_eprintln!("⚠️ Cannot compress even at max CRF {:.1}!", max_crf);
            crate::log_eprintln!("   File may be already optimally compressed");
            // Use best tested CRF instead of fallback to max_crf
            if best_crf.is_none() {
                best_crf = Some(best_tested_crf);
                best_size = Some(best_tested_size);
            }
        } else {
            crate::log_eprintln!();
            crate::log_eprintln!(
                "{}Phase 3: [CPU] Search DOWNWARD with Sprint & Backtrack (min step {:.2}){}",
                BRIGHT_CYAN,
                PHASE3_DOWNWARD_STEP,
                RESET
            );

            let compress_point = best_crf.unwrap_or(gpu_boundary_crf);
            let mut current_step = PHASE3_DOWNWARD_STEP;
            let mut failure_credibility = 0.0f64;
            let mut consecutive_failures = 0u32;
            let mut consecutive_01_successes = 0u32;
            let mut prev_ssim_opt = best_ssim_tracked;
            let mut prev_size = best_size.unwrap_or(0);
            let search_floor = if ultimate_mode { 0.0 } else { min_crf };
            let mut test_crf = compress_point - current_step;

            while test_crf >= search_floor && iterations < max_iterations_for_video {
                if size_cache.contains_key(test_crf) {
                    test_crf -= current_step;
                    continue;
                }

                let size = encode_cached(test_crf, &mut size_cache)?;
                iterations += 1;
                let total_size_pct = if input_size > 0 {
                    (size as f64 / input_size as f64 - 1.0) * 100.0
                } else {
                    0.0
                };
                
                let current_ssim_opt = calculate_ssim_quick();
                
                // Ultimate metrics for insight mechanism
                let mut vmaf_improved = false;
                let mut psnr_improved = false;
                let mut metrics_info = String::new();
                let mut current_vmaf_val = None;
                let mut current_psnr_val = None;

                if ultimate_mode {
                    let vmaf = super::ssim_calculator::calculate_vmaf_y(input, output, 6);
                    let psnr_uv = super::ssim_calculator::calculate_psnr_uv(input, output, 6);
                    
                    if let (Some(v), Some((u, v_score))) = (vmaf, psnr_uv) {
                        let chroma_avg = (u + v_score) / 2.0;
                        let prev_best_vmaf = best_vmaf_tracked.unwrap_or(0.0);
                        let prev_best_psnr = best_psnr_uv_tracked.map(|(u,v)| (u+v)/2.0).unwrap_or(0.0);

                        vmaf_improved = v.floor() > prev_best_vmaf.floor();
                        psnr_improved = chroma_avg.floor() > prev_best_psnr.floor();
                        
                        metrics_info = format!("VMAF:{:.2} UV:{:.2}", v, chroma_avg);
                        
                        current_vmaf_val = Some(v);
                        current_psnr_val = Some((u, v_score));
                    }
                }

                let is_effectively_compressed = size < input_size;

                if is_effectively_compressed {
                    consecutive_failures = 0;

                    best_crf = Some(test_crf);
                    best_size = Some(size);
                    best_ssim_tracked = current_ssim_opt;
                    
                    if ultimate_mode {
                        if vmaf_improved || best_vmaf_tracked.is_none() { *best_vmaf_tracked = current_vmaf_val; }
                        if psnr_improved || best_psnr_uv_tracked.is_none() { *best_psnr_uv_tracked = current_psnr_val; }
                    }

                    let size_increase = size as i64 - prev_size as i64;
                    let _size_increase_pct = if prev_size > 0 {
                        (size_increase as f64 / prev_size as f64) * 100.0
                    } else {
                        0.0
                    };

                    let improvement_indicator = if vmaf_improved || psnr_improved { "↑" } else { "→" };
                    let _insight_msg = if ultimate_mode { 
                        format!("{} (Index: {:.0}/3) {}", metrics_info, failure_credibility, improvement_indicator)
                    } else { 
                        String::new()
                    };

                    let ssim_gain = match (current_ssim_opt, prev_ssim_opt) {
                        (Some(curr), Some(prev)) => curr - prev,
                        _ => 0.0,
                    };
                    
                    let metrics_str = if ultimate_mode {
                        let v_val = *best_vmaf_tracked;
                        let uv_val = *best_psnr_uv_tracked;
                        if let (Some(v), Some((u, v_score))) = (v_val, uv_val) {
                            let chroma_avg = (u + v_score) / 2.0;
                            format!(" │ VMAF:{:.2} UV:{:.2} ({:.0}/3 {})", v, chroma_avg, failure_credibility, improvement_indicator)
                        } else {
                            String::new()
                        }
                    } else if let Some(current_ssim) = current_ssim_opt {
                        format!(" │ SSIM:{:.4} Δ{:+.4}", current_ssim, ssim_gain)
                    } else {
                        " │ SSIM N/A".to_string()
                    };

                    crate::log_eprintln!(
                        "{}{}   {}✓{} [CPU] {}CRF {:<4.1}{} {}{:6.1}%{}{} (step {:.2}) ✅",
                        RESET, RESET, BRIGHT_GREEN, RESET, CYAN, test_crf, RESET,
                        BRIGHT_GREEN, total_size_pct, RESET, metrics_str, current_step
                    );

                    // Early termination logic: based on insight evaluation
                    if ultimate_mode {
                        // Only trigger if quality already meets final settlement thresholds
                        const VMAF_Y_MIN: f64 = 93.0;
                        const PSNR_UV_MIN: f64 = 38.0;

                        let any_metric_fails = if let (Some(v), Some(uv)) = (current_vmaf_val, current_psnr_val) {
                            v < VMAF_Y_MIN || uv.0.min(uv.1) < PSNR_UV_MIN
                        } else {
                            false
                        };

                        if !vmaf_improved && !psnr_improved && any_metric_fails {
                            failure_credibility += 1.0;
                            if failure_credibility >= 3.0 {
                                crate::log_eprintln!(
                                    "   {}❌ QUALITY PLATEAU REACHED (3/3):{} No integer improvement over 3 insights. Stopping.",
                                    BRIGHT_RED, RESET
                                );
                                break;
                            }
                        } else {
                            failure_credibility = 0.0;
                        }
                    } else {
                        // Original non-ultimate mode stopping logic
                        if let (Some(s), Some(p)) = (current_ssim_opt, prev_ssim_opt) {
                            if s - p < 0.0001 && s >= 0.99 {
                                crate::log_eprintln!("   {}SSIM plateau → STOP{}", BRIGHT_CYAN, RESET);
                                break;
                            }
                        }
                    }

                    prev_ssim_opt = current_ssim_opt;
                    prev_size = size;

                    // Sprint: double the step for faster iteration (after 2 consecutive successes)
                    if current_step <= PHASE3_DOWNWARD_STEP + 0.01 {
                        consecutive_01_successes += 1;
                    } else if consecutive_01_successes >= 2 {
                        consecutive_01_successes += 1;
                    } else {
                        consecutive_01_successes = 0;
                    }

                    if consecutive_01_successes >= 2 && current_step < 1.6 {
                        let old_step = current_step;
                        current_step = (current_step * 2.0).min(1.6);
                        crate::verbose_eprintln!(
                            "   {}SPRINT:{} {:.2} → {:.2} (accelerated search)",
                            BRIGHT_CYAN, RESET, old_step, current_step
                        );
                    }

                    test_crf -= current_step;
                } else {
                    consecutive_failures += 1;
                    
                    let metrics_str = if ultimate_mode {
                        let v_val = *best_vmaf_tracked;
                        let uv_val = *best_psnr_uv_tracked;
                        if let (Some(v), Some((u, v_score))) = (v_val, uv_val) {
                            let chroma_avg = (u + v_score) / 2.0;
                            format!(" │ VMAF:{:.2} UV:{:.2} ({:.0}/3 →)", v, chroma_avg, failure_credibility)
                        } else {
                            String::new()
                        }
                    } else {
                        String::new()
                    };

                    crate::log_eprintln!(
                        "{}{}   {}✗{} [CPU] {}CRF {:<4.1}{} {}{:6.1}%{}{} {}❌ (fail #{}/{})",
                        RESET, RESET, BRIGHT_RED, RESET, CYAN, test_crf, RESET,
                        BRIGHT_RED, total_size_pct, RESET, metrics_str, BRIGHT_RED, consecutive_failures, MAX_CONSECUTIVE_FAILURES
                    );

                    // Backtrack: if we were sprinting and hit a wall, reset to precision mode
                    if current_step > PHASE3_DOWNWARD_STEP + 0.01 && consecutive_01_successes >= 2 {
                        let old_step = current_step;
                        current_step = PHASE3_DOWNWARD_STEP;
                        consecutive_01_successes = 0;
                        crate::log_eprintln!(
                            "   {}BACKTRACK:{} {:.2} → {:.2} (overshoot correction)",
                            BRIGHT_YELLOW, RESET, old_step, current_step
                        );
                        test_crf = compress_point - current_step;
                        continue;
                    }

                    // If not ultimate mode, immediately break on first capacity exceed in phase 3
                    if !ultimate_mode {
                        crate::log_eprintln!(
                            "   {}Capacity exceeded at step {:.2}. Stopping.{}",
                            BRIGHT_YELLOW,
                            PHASE3_DOWNWARD_STEP,
                            RESET
                        );
                        break;
                    }

                    // For ultimate mode, continue stepping down to see if quality metric overrides or recovers
                    current_step = PHASE3_DOWNWARD_STEP;
                    test_crf -= current_step;

                    // Insight mechanism also applies to failures
                    if ultimate_mode {
                        failure_credibility += 1.0;
                        if failure_credibility >= 3.0 {
                            crate::log_eprintln!(
                                "   {}❌ FAILURE CREDIBILITY REACHED (3/3):{} Sustained failure/quality collapse. Stopping.",
                                BRIGHT_RED, RESET
                            );
                            early_insight_triggered = true;
                            break;
                        }
                    }

                    if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                        crate::log_eprintln!(
                            "   {}Max consecutive failures ({}) → STOP{}",
                            BRIGHT_RED,
                            MAX_CONSECUTIVE_FAILURES,
                            RESET
                        );
                        break;
                    }
                }
            }
        }
    }

    if ultimate_mode && !early_insight_triggered {
        if let Some(best) = best_crf {
            if best < max_crf {
                crate::log_eprintln!();
                use crate::modern_ui::colors::*;
                crate::log_eprintln!(
                    "{}Phase 4: [CPU] Extreme Mode 0.01-Granularity Fine-Tune{}",
                    BRIGHT_MAGENTA, RESET
                );
                crate::log_eprintln!(
                    "   {}Starting from 0.1 optimum (CRF {:.2}) and stepping downwards by 0.01{}",
                    DIM, best, RESET
                );

                let mut current_best = best;
                let mut current_best_size = best_size.unwrap_or(0);
                
                let fine_step = 0.01;
                let mut test_crf = best - fine_step;
                let mut fine_failures = 0;
                let max_fine_failures = 3;
                let search_floor = 0.0;

                while test_crf >= search_floor && iterations < 200 {
                    // Preempt float precision issues
                    test_crf = (test_crf * 100.0).round() / 100.0;
                    
                    if size_cache.contains_key(test_crf) {
                        test_crf -= fine_step;
                        continue;
                    }
                    
                    let size = encode_cached(test_crf, &mut size_cache)?;
                    iterations += 1;
                    
                    let is_effectively_compressed = size < input_size;
                    let total_size_pct = if input_size > 0 {
                        (size as f64 / input_size as f64 - 1.0) * 100.0
                    } else {
                        0.0
                    };
                    
                    if is_effectively_compressed {
                        current_best = test_crf;
                        current_best_size = size;
                        best_ssim_tracked = calculate_ssim_quick(); // track latest if valid
                        fine_failures = 0;
                        
                        crate::log_eprintln!(
                            "{}{}   {}✓{} [CPU] {}CRF {:<5.2}{} {}{:6.1}%{} │ 0.01-GRANULARITY GAIN",
                            RESET, RESET, BRIGHT_GREEN, RESET, CYAN, test_crf, RESET,
                            BRIGHT_GREEN, total_size_pct, RESET
                        );
                    } else {
                        fine_failures += 1;
                        crate::log_eprintln!(
                            "{}{}   {}✗{} [CPU] {}CRF {:<5.2}{} {}{:6.1}%{} │ CAPACITY EXCEEDED",
                            RESET, RESET, BRIGHT_RED, RESET, CYAN, test_crf, RESET,
                            BRIGHT_RED, total_size_pct, RESET
                        );
                        
                        if fine_failures >= max_fine_failures {
                            crate::log_eprintln!("   {}Max 0.01-granularity failures reached. Stopping.{}", BRIGHT_YELLOW, RESET);
                            break;
                        }
                    }
                    test_crf -= fine_step;
                }
                
                best_crf = Some(current_best);
                best_size = Some(current_best_size);
            }
        }
    }

    let (final_crf, final_full_size) = match (best_crf, best_size) {
        (Some(crf), Some(size)) if crf < max_crf => {
            crate::log_eprintln!("{}✅ Best CRF {:.2} already encoded (full video){}", BRIGHT_GREEN, crf, RESET);
            (crf, size)
        }
        _ => {
            if early_insight_triggered {
                crate::log_eprintln!("{}⚠️  Skipping final settlement: early insight already proved further compression is futile.{}", BRIGHT_YELLOW, RESET);
                let fallback_crf = max_crf;
                // Use input_size + 1 to ensure it registers as uncompressed 
                // since we know any compression destroys quality based on insight.
                let size = input_size + 1;
                (fallback_crf, size)
            } else {
                crate::log_eprintln!("{}⚠️  Fallback: using max CRF {:.2} (no better compression found){}", BRIGHT_YELLOW, max_crf, RESET);

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
        }
    };

    crate::verbose_eprintln!(
        "Final: CRF {:.2} | Size: {} bytes ({:.2} MB)",
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

    // User-relevant success: total file smaller (with 1MB tolerance if allowed) and quality met
    let size_tolerance = if allow_size_tolerance { 1_048_576 } else { 0 };
    let total_file_compressed = final_full_size <= input_size + size_tolerance;
    let _video_stream_compressed =
        crate::stream_size::can_compress_pure_video(output, input_video_stream_size, true);
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
        "[FINISH] RESULT: CRF {:.2} │ Size {:+.1}% │ Iterations: {}",
        final_crf,
        size_change_pct,
        iterations
    );
    crate::log_eprintln!(
        "   Total file smaller than input: {}",
        if total_file_compressed {
            "YES"
        } else {
            "NO"
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

    // Detect animated image formats (GIF, WebP, AVIF) and use relaxed duration tolerance
    let is_animated_image = input
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            let ext = e.to_lowercase();
            matches!(ext.as_str(), "gif" | "webp" | "avif" | "heic" | "heif")
        })
        .unwrap_or(false);

    let verify_options = if is_animated_image {
        crate::verbose_eprintln!("   🎞️  Animated image detected, using relaxed duration tolerance");
        crate::quality_verifier_enhanced::VerifyOptions::relaxed_animated_image()
    } else {
        crate::quality_verifier_enhanced::VerifyOptions::strict_video()
    };

    let enhanced = crate::quality_verifier_enhanced::verify_after_encode(
        input,
        output,
        &verify_options,
    );
    crate::verbose_eprintln!("   {}", enhanced.summary());
    for d in &enhanced.details {
        crate::verbose_eprintln!("      {}", d);
    }
    let enhanced_verify_fail_reason = if enhanced.passed() {
        None
    } else {
        Some(enhanced.message.clone())
    };
    let quality_passed = quality_passed && enhanced.passed();

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
        enhanced_verify_fail_reason,
        log,
        confidence,
        confidence_detail,
        actual_min_ssim: min_ssim,
        input_video_stream_size: input_stream_info.video_stream_size,
        output_video_stream_size: output_stream_info.video_stream_size,
        container_overhead: output_stream_info.container_overhead,
        vmaf_y_score: None,
        cambi_score: None,
        psnr_uv_score: None,
        early_insight_triggered,
    })
}

pub fn explore_hevc_with_gpu_coarse(
    input: &Path,
    output: &Path,
    vf_args: Vec<String>,
    initial_crf: f32,
    allow_size_tolerance: bool,
    max_threads: usize,
) -> Result<ExploreResult> {
    let (_, min_ssim) = calculate_smart_thresholds(initial_crf, VideoEncoder::Hevc);
    explore_hevc_with_gpu_coarse_full(
        input,
        output,
        vf_args,
        initial_crf,
        false,
        false,
        allow_size_tolerance,
        min_ssim,
        max_threads,
    )
}

pub fn explore_hevc_with_gpu_coarse_ultimate(
    input: &Path,
    output: &Path,
    vf_args: Vec<String>,
    initial_crf: f32,
    ultimate_mode: bool,
    allow_size_tolerance: bool,
    max_threads: usize,
) -> Result<ExploreResult> {
    let (_, min_ssim) = calculate_smart_thresholds(initial_crf, VideoEncoder::Hevc);
    explore_hevc_with_gpu_coarse_full(
        input,
        output,
        vf_args,
        initial_crf,
        ultimate_mode,
        false,
        allow_size_tolerance,
        min_ssim,
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
    allow_size_tolerance: bool,
    min_ssim: f64,
    max_threads: usize,
) -> Result<ExploreResult> {
    let (max_crf, _) = calculate_smart_thresholds(initial_crf, VideoEncoder::Hevc);
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
        allow_size_tolerance,
        max_threads,
    )
}

pub fn explore_av1_with_gpu_coarse(
    input: &Path,
    output: &Path,
    vf_args: Vec<String>,
    initial_crf: f32,
    allow_size_tolerance: bool,
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
        allow_size_tolerance,
        max_threads,
    )
}

pub fn explore_av1_with_gpu_coarse_ultimate(
    input: &Path,
    output: &Path,
    vf_args: Vec<String>,
    initial_crf: f32,
    ultimate_mode: bool,
    allow_size_tolerance: bool,
    max_threads: usize,
) -> Result<ExploreResult> {
    let (_, min_ssim) = calculate_smart_thresholds(initial_crf, VideoEncoder::Av1);
    explore_av1_with_gpu_coarse_full(
        input,
        output,
        vf_args,
        initial_crf,
        ultimate_mode,
        false,
        allow_size_tolerance,
        min_ssim,
        max_threads,
    )
}

pub fn explore_av1_with_gpu_coarse_full(
    input: &Path,
    output: &Path,
    vf_args: Vec<String>,
    initial_crf: f32,
    ultimate_mode: bool,
    force_ms_ssim_long: bool,
    allow_size_tolerance: bool,
    min_ssim: f64,
    max_threads: usize,
) -> Result<ExploreResult> {
    let (max_crf, _) = calculate_smart_thresholds(initial_crf, VideoEncoder::Av1);
    explore_with_gpu_coarse_search(
        input,
        output,
        VideoEncoder::Av1,
        vf_args,
        initial_crf,
        max_crf,
        min_ssim,
        ultimate_mode,
        force_ms_ssim_long,
        allow_size_tolerance,
        max_threads,
    )
}
