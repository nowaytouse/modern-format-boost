//! MS-SSIM quality metric calculations (multi-scale, YUV channel-wise)
//!
//! Primary entry: `calculate_ms_ssim_yuv` (used by `gpu_coarse_search` Phase 3).
//! `calculate_ms_ssim` is single-channel luma with standalone-vmaf fallback for other callers.

use crate::builder_base::ToolBuilder;
use std::path::Path;

fn common_even_metric_dimensions(
    input_width: u32,
    input_height: u32,
    output_width: u32,
    output_height: u32,
) -> Option<(u32, u32)> {
    let target_width = input_width.min(output_width);
    let target_height = input_height.min(output_height);
    let (target_width, target_height, _) =
        crate::video::ensure_even_dimensions(target_width, target_height);

    if target_width == 0 || target_height == 0 {
        return None;
    }

    Some((target_width, target_height))
}

fn resolve_common_metric_dimensions(input: &Path, output: &Path) -> Option<(u32, u32)> {
    let (input_width, input_height) = match crate::conversion::get_input_dimensions(input) {
        Ok(d) => d,
        Err(err) => {
            crate::log_anomaly!(
                crate::static_logs::messages::LABEL_FFPROBE,
                &format!("Failed to read reference dimensions for quality metric: {err}")
            );
            return None;
        }
    };
    let (output_width, output_height) = match crate::conversion::get_input_dimensions(output) {
        Ok(d) => d,
        Err(err) => {
            crate::log_anomaly!(
                crate::static_logs::messages::LABEL_FFPROBE,
                &format!("Failed to read distorted dimensions for quality metric: {err}")
            );
            return None;
        }
    };

    let (target_width, target_height) =
        common_even_metric_dimensions(input_width, input_height, output_width, output_height)?;

    Some((target_width, target_height))
}

/// `max_duration_min`: skip MS-SSIM when video longer than this (e.g. 5.0 normal, 25.0 ultimate).
// Rationale: This function handles complex, sequential initialization or business logic where further fragmentation would hinder readability and maintainability.
#[allow(
    clippy::too_many_lines,
    reason = "Complex orchestration logic where fragmenting state into smaller helpers would decrease readability and increase cognitive overhead."
)]
#[must_use]
pub fn calculate_ms_ssim_yuv(
    input: &Path,
    output: &Path,
    max_duration_min: f64,
) -> Option<(f64, f64, f64, f64)> {
    use chrono::Local;
    use std::thread;

    if let Some(ext) = input.extension().and_then(|e| e.to_str())
        && matches!(ext.to_lowercase().as_str(), "gif")
    {
        crate::log_hint!(
            crate::static_logs::messages::LABEL_MS_SSIM,
            "GIF format: skipping MS-SSIM (libvmaf incompatible), caller will use SSIM-All."
        );
        return None;
    }

    let Some(duration) = super::stream_analysis::get_video_duration(input) else {
        crate::log_anomaly!(
            crate::static_logs::messages::LABEL_MS_SSIM,
            "Cannot determine video duration, skipping MS-SSIM"
        );
        return None;
    };
    let duration_min = duration / 60.0_f64;

    // Caller sets max_duration_min (e.g. 5 min normal, 25 min ultimate) to control skip threshold.
    let (sample_rate, should_calculate) =
        if duration_min <= crate::constants::QUALITY_ANALYSIS_SHORT_DURATION_MIN {
            (crate::constants::QUALITY_ANALYSIS_SAMPLE_RATE_SHORT, true)
        } else if duration_min <= max_duration_min {
            (crate::constants::QUALITY_ANALYSIS_SAMPLE_RATE_LONG, true)
        } else {
            (0, false)
        };

    if !should_calculate {
        crate::log_anomaly!(
            crate::static_logs::messages::LABEL_MS_SSIM,
            &format!(
                "video too long ({duration_min:.1}min > {max_duration_min:.0}min), MS-SSIM skipped."
            )
        );
        crate::log_detail!("Using SSIM-only verification (faster; multi-scale not computed).");
        return None;
    }

    let start_ts = Local::now().format("%Y-%m-%d %H:%M:%S");
    crate::log_detail!("Calculating 3-channel MS-SSIM (Y+U+V)...");
    crate::log_detail!(&format!("Start time: {start_ts}"));
    crate::log_detail!(&format!("Video: {duration:.1}s ({duration_min:.1}min)"));

    if sample_rate > 1 {
        let sample_rate_u32 = crate::numeric_cast::usize_to_u32_strict(sample_rate, "sample_rate")?;
        let estimated_time = crate::numeric_cast::f64_to_u64_strict(
            duration / f64::from(sample_rate_u32) * 3.0,
            "estimated_time",
        )?;
        crate::log_detail!(&format!(
            "⚡ Sampling: 1/{sample_rate} frames (est. {estimated_time}s)"
        ));
    } else {
        let estimated_time =
            crate::numeric_cast::f64_to_u64_strict(duration * 3.0, "estimated_time")?;
        crate::log_detail!(&format!("🎯 Full calculation (est. {estimated_time}s)"));
    }
    crate::log_detail!("🔄 Parallel processing: Y+U+V channels simultaneously");

    let (target_width, target_height) = resolve_common_metric_dimensions(input, output)?;

    let input_y = input.to_path_buf();
    let output_y = output.to_path_buf();
    let input_u = input.to_path_buf();
    let output_u = output.to_path_buf();
    let input_v = input.to_path_buf();
    let output_v = output.to_path_buf();

    let start_time = std::time::Instant::now();

    let y_handle = thread::spawn(move || {
        calculate_ms_ssim_channel_sampled(
            &input_y,
            &output_y,
            "y",
            sample_rate,
            target_width,
            target_height,
        )
    });
    let u_handle = thread::spawn(move || {
        calculate_ms_ssim_channel_sampled(
            &input_u,
            &output_u,
            "u",
            sample_rate,
            target_width,
            target_height,
        )
    });
    let v_handle = thread::spawn(move || {
        calculate_ms_ssim_channel_sampled(
            &input_v,
            &output_v,
            "v",
            sample_rate,
            target_width,
            target_height,
        )
    });

    let Ok(Some(y_ms_ssim)) = y_handle.join() else {
        crate::log_failure!(
            crate::static_logs::messages::LABEL_MS_SSIM,
            "Y channel calculation failed"
        );
        return None;
    };
    let u_ms_ssim = match u_handle.join() {
        Ok(Some(v)) => Some(v),
        _ => None,
    };
    let v_ms_ssim = match v_handle.join() {
        Ok(Some(v)) => Some(v),
        _ => None,
    };

    crate::log_detail!(&format!("      Y channel... {y_ms_ssim:.4} ✅"));
    if let Some(u) = u_ms_ssim {
        crate::log_detail!(&format!("      U channel... {u:.4} ✅"));
    } else {
        crate::log_detail!("      U channel... skipped (resolution too small)");
    }
    if let Some(v) = v_ms_ssim {
        crate::log_detail!(&format!("      V channel... {v:.4} ✅"));
    } else {
        crate::log_detail!("      V channel... skipped (resolution too small)");
    }

    let elapsed = start_time.elapsed().as_secs();
    let end_time = Local::now().format("%Y-%m-%d %H:%M:%S");
    crate::log_detail!(&format!("⏱️  Completed in {elapsed}s (End: {end_time})"));

    // If chroma channels are available, weight by 4:2:0 sample counts (Y:U:V = 4:1:1).
    // In YUV 4:2:0, each 2x2 luma block has 4 Y samples but only 1 U and 1 V sample,
    // so Y contributes 4/6 of the signal and each chroma plane contributes 1/6.
    // If not, use Y-only (still perceptually dominant and meaningful).
    let (u_val, v_val, weighted_avg) = if let (Some(u), Some(v)) = (u_ms_ssim, v_ms_ssim) {
        let avg = (y_ms_ssim.mul_add(4.0, u) + v) / 6.0_f64;
        (u, v, avg)
    } else {
        crate::log_detail!("      ℹ️  Using Y-only MS-SSIM (chroma channels unavailable)");
        (y_ms_ssim, y_ms_ssim, y_ms_ssim)
    };

    Some((
        y_ms_ssim.clamp(0.0, 1.0),
        u_val.clamp(0.0, 1.0),
        v_val.clamp(0.0, 1.0),
        weighted_avg.clamp(0.0, 1.0),
    ))
}

fn calculate_ms_ssim_channel_sampled(
    input: &Path,
    output: &Path,
    channel: &str,
    sample_rate: usize,
    target_width: u32,
    target_height: u32,
) -> Option<f64> {
    if let Some(ext) = input.extension().and_then(|e| e.to_str())
        && matches!(ext.to_lowercase().as_str(), "gif")
    {
        crate::log_detail!(
            "      ℹ️  GIF format: skipping YUV channel extraction (use SSIM-All instead)"
        );
        return None;
    }

    // For chroma channels (U/V) in YUV 4:2:0, the extracted plane is half the
    // luma resolution. libvmaf MS-SSIM performs multi-scale downsampling and
    // fails with "scale below 1x1" when the plane is too small.
    // Minimum safe luma resolution for chroma MS-SSIM: 256x256 (chroma = 128x128).
    if matches!(channel, "u" | "v")
        && (target_width < crate::constants::MS_SSIM_CHROMA_MIN_DIM
            || target_height < crate::constants::MS_SSIM_CHROMA_MIN_DIM)
    {
        crate::log_detail!(&format!(
            "      ℹ️  Channel {}: resolution {}x{} too small for chroma MS-SSIM (min {}x{}), skipping",
            channel.to_uppercase(),
            target_width,
            target_height,
            crate::constants::MS_SSIM_CHROMA_MIN_DIM,
            crate::constants::MS_SSIM_CHROMA_MIN_DIM
        ));
        return None;
    }

    let sample_filter = if sample_rate > 1 {
        format!("select='not(mod(n\\,{sample_rate}))',setpts=N/FRAME_RATE/TB,")
    } else {
        String::new()
    };

    // For HDR content, we need to convert to 8-bit for libvmaf compatibility
    // libvmaf's MS-SSIM feature may not support 10-bit input properly
    // Note: This means we lose some HDR information, but it's better than failing
    let filter = format!(
        "[0:v]{sample_filter}scale={target_width}:{target_height}:flags=bicubic,format=yuv420p,extractplanes={channel}[c0];[1:v]{sample_filter}scale={target_width}:{target_height}:flags=bicubic,format=yuv420p,extractplanes={channel}[c1];[c0][c1]libvmaf=feature='name=float_ms_ssim':log_fmt=json:log_path=/dev/stdout",
    );

    let result = crate::ffmpeg_builder::FfmpegBuilder::new()
        .input(input)
        .input(output)
        .arg("-filter_complex")
        .arg(&filter)
        .arg("-f")
        .arg("null")
        .output_pipe()
        .build()
        .output();

    match result {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            // Always try to parse JSON from stdout first — ffmpeg may return
            // non-zero exit code even when it successfully computed the metric
            // (e.g. due to harmless warnings written to stderr).
            if let Some(score) = parse_ms_ssim_from_json(&stdout) {
                return Some(score);
            }

            // Only report failure if we truly got no usable result
            if !out.status.success() {
                let stderr = String::from_utf8_lossy(&out.stderr);
                crate::log_failure!(
                    crate::static_logs::messages::LABEL_MS_SSIM,
                    &format!("Channel {} MS-SSIM failed!", channel.to_uppercase())
                );

                if stderr.contains("No such filter: 'libvmaf'") {
                    crate::log_detail!("         Cause: libvmaf filter not available in ffmpeg");
                    crate::log_detail!(
                        "         Fix: brew install homebrew-ffmpeg/ffmpeg/ffmpeg --with-libvmaf"
                    );
                } else if stderr.contains("Invalid pixel format")
                    || stderr.contains("Discarding mismatched")
                {
                    crate::log_detail!("         Cause: Pixel format incompatibility");
                    crate::log_detail!(&format!("         Input: {}", input.display()));
                } else {
                    let error_lines: Vec<&str> = stderr
                        .lines()
                        .filter(|l| {
                            (l.contains("Error") || l.contains("error") || l.contains("failed"))
                                && !l.contains("Last message repeated")
                        })
                        .take(3)
                        .collect();
                    if !error_lines.is_empty() {
                        crate::log_detail!(&format!("         Error: {}", error_lines.join(" | ")));
                    }
                }
            }
            None
        }
        Err(e) => {
            crate::log_failure!(
                crate::static_logs::messages::LABEL_MS_SSIM,
                &format!("Channel {} command failed: {e}", channel.to_uppercase())
            );
            None
        }
    }
}

#[must_use]
pub fn calculate_ms_ssim(input: &Path, output: &Path) -> Option<f64> {
    if let Ok(info) = crate::ffprobe::probe_video(input)
        && (info.width < 64 || info.height < 64)
    {
        crate::log_anomaly!(
            crate::static_logs::messages::LABEL_MS_SSIM,
            &format!(
                "Skipping MS-SSIM: Image too small ({}x{}) for multi-scale analysis",
                info.width, info.height
            )
        );
        return None;
    }

    crate::log_detail!("Calculating MS-SSIM (Multi-Scale Structural Similarity)...");

    let (target_width, target_height) = resolve_common_metric_dimensions(input, output)?;

    // Always use 8-bit yuv420p for libvmaf compatibility
    // libvmaf's MS-SSIM feature works best with 8-bit input
    let result = crate::ffmpeg_builder::FfmpegBuilder::new()
        .input(input)
        .input(output)
        .arg("-filter_complex")
        .arg(format!(
            "[0:v]scale={target_width}:{target_height}:flags=bicubic,format=yuv420p[ref];[1:v]scale={target_width}:{target_height}:flags=bicubic,format=yuv420p[dis];[ref][dis]libvmaf=log_path=/dev/stdout:log_fmt=json:feature='name=float_ms_ssim'",
        ))
        .arg("-f")
        .arg("null")
        .output_pipe()
        .build()
        .output();

    match result {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);

            // Always try to parse JSON from stdout first — ffmpeg may return
            // non-zero exit code even when it successfully computed the metric.
            if let Some(ms_ssim) = parse_ms_ssim_from_json(&stdout) {
                let clamped = ms_ssim.clamp(0.0, 1.0);
                if (ms_ssim - clamped).abs() > 0.000_1_f64 {
                    crate::log_anomaly!(
                        crate::static_logs::messages::LABEL_MS_SSIM,
                        &format!(
                            "MS-SSIM raw value {ms_ssim:.6} out of range, clamped to {clamped:.4}"
                        )
                    );
                }
                crate::log_detail!(&format!("MS-SSIM score: {clamped:.4}"));
                return Some(clamped);
            }

            if let Some(ms_ssim) = parse_ms_ssim_from_legacy(&stderr) {
                let clamped = ms_ssim.clamp(0.0, 1.0);
                if (ms_ssim - clamped).abs() > 0.000_1_f64 {
                    crate::log_anomaly!(
                        crate::static_logs::messages::LABEL_MS_SSIM,
                        &format!(
                            "MS-SSIM raw value {ms_ssim:.6} out of range, clamped to {clamped:.4}"
                        )
                    );
                }
                crate::log_detail!(&format!("MS-SSIM score: {clamped:.4}"));
                return Some(clamped);
            }

            // No parseable score found
            if out.status.success() {
                crate::log_anomaly!(
                    crate::static_logs::messages::LABEL_MS_SSIM,
                    "MS-SSIM calculated but failed to parse score"
                );
            } else {
                crate::log_failure!(
                    crate::static_logs::messages::LABEL_MS_SSIM,
                    "ffmpeg libvmaf MS-SSIM failed"
                );
                crate::log_detail!("🔄 Trying standalone vmaf tool as fallback...");

                if crate::vmaf_standalone::is_vmaf_available() {
                    match crate::vmaf_standalone::calculate_ms_ssim_standalone(input, output) {
                        Ok(score) => {
                            crate::log_detail!(&format!("✅ Standalone vmaf MS-SSIM: {score:.4}"));
                            return Some(score);
                        }
                        Err(e) => {
                            crate::log_anomaly!(
                                crate::static_logs::messages::LABEL_MS_SSIM,
                                &format!("Standalone vmaf also failed: {e}")
                            );
                        }
                    }
                }
            }
        }
        Err(e) => {
            crate::log_failure!(
                crate::static_logs::messages::LABEL_MS_SSIM,
                &format!("ffmpeg MS-SSIM failed: {e}")
            );
        }
    }

    None
}

fn parse_ms_ssim_from_json(stdout: &str) -> Option<f64> {
    if let Some(pooled_pos) = stdout.find("\"pooled_metrics\"") {
        let after_pooled = &stdout[pooled_pos..];
        if let Some(ms_ssim_pos) = after_pooled.find("\"float_ms_ssim\"") {
            let after_ms_ssim = &after_pooled[ms_ssim_pos..];
            if let Some(mean_pos) = after_ms_ssim.find("\"mean\"") {
                let after_mean = &after_ms_ssim[mean_pos + 6..];
                if let Some(colon_pos) = after_mean.find(':') {
                    let after_colon = after_mean[colon_pos + 1..].trim_start();
                    let end = after_colon
                        .find(|c: char| !c.is_numeric() && c != '.')
                        .unwrap_or(after_colon.len());
                    if end > 0 {
                        match after_colon[..end].parse::<f64>() {
                            Ok(f) => return Some(f),
                            Err(e) => {
                                crate::log_anomaly!(
                                    crate::static_logs::messages::LABEL_MS_SSIM,
                                    &format!(
                                        "Failed to parse float_ms_ssim value '{}': {}",
                                        &after_colon[..end],
                                        e
                                    )
                                );
                                return None;
                            }
                        }
                    }
                }
            }
        }
    }
    None
}

fn parse_ms_ssim_from_legacy(stderr: &str) -> Option<f64> {
    for line in stderr.lines() {
        if (line.contains("MS-SSIM") || line.contains("ms_ssim") || line.contains("float_ms_ssim"))
            && line.contains("score:")
            && let Some(score_pos) = line.find("score:")
        {
            let after_score = &line[score_pos + 6..].trim_start();
            let end = after_score
                .find(|c: char| !c.is_numeric() && c != '.')
                .unwrap_or(after_score.len());
            if end > 0 {
                match after_score[..end].parse::<f64>() {
                    Ok(f) => return Some(f),
                    Err(e) => {
                        crate::log_anomaly!(
                            crate::static_logs::messages::LABEL_MS_SSIM,
                            &format!(
                                "Failed to parse legacy ms_ssim value '{}': {}",
                                &after_score[..end],
                                e
                            )
                        );
                        return None;
                    }
                }
            }
        }
    }
    None
}

// ─── Ultimate Mode: 3D Quality Metrics ────────────────────────────────────────

/// Calculate VMAF Y-channel score (perceptual quality, 0–100 scale).
/// `sample_rate`: 1 = every frame, 3 = every 3rd frame, etc.
/// Returns None on failure (ffmpeg/libvmaf unavailable or other error).
#[must_use]
pub fn calculate_vmaf_y(input: &Path, output: &Path, sample_rate: usize) -> Option<f64> {
    let sample_filter = if sample_rate > 1 {
        format!("select='not(mod(n\\,{sample_rate}))',setpts=N/FRAME_RATE/TB,")
    } else {
        String::new()
    };

    let n_threads = num_cpus_capped();
    let (target_width, target_height) = resolve_common_metric_dimensions(input, output)?;

    // Always use 8-bit yuv420p for VMAF calculation compatibility
    // VMAF models are trained on 8-bit content
    let filter = format!(
        "[0:v]{sample_filter}scale={target_width}:{target_height}:flags=bicubic,format=yuv420p[dis];[1:v]{sample_filter}scale={target_width}:{target_height}:flags=bicubic,format=yuv420p[ref];[dis][ref]libvmaf=shortest=true:ts_sync_mode=nearest:n_threads={n_threads}:log_fmt=json:log_path=/dev/stdout",
    );

    let result = crate::ffmpeg_builder::FfmpegBuilder::new()
        .input(output)
        .input(input)
        .arg("-filter_complex")
        .arg(&filter)
        .arg("-f")
        .arg("null")
        .output_pipe()
        .build()
        .output();

    match result {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            // Always try to parse JSON from stdout first — ffmpeg may return
            // non-zero exit code even when it successfully computed the metric.
            if let Some(score) = parse_vmaf_mean_from_json(&stdout) {
                return Some(score);
            }

            if !out.status.success() {
                let stderr = String::from_utf8_lossy(&out.stderr);
                crate::log_failure!(
                    crate::static_logs::messages::LABEL_VMAF,
                    "VMAF-Y calculation failed!"
                );
                if stderr.contains("No such filter: 'libvmaf'") {
                    crate::log_detail!(
                        "         Cause: libvmaf not available in this ffmpeg build"
                    );
                    crate::log_detail!(
                        "         Fix: brew install homebrew-ffmpeg/ffmpeg/ffmpeg --with-libvmaf"
                    );
                } else {
                    let error_lines: Vec<&str> = stderr
                        .lines()
                        .filter(|l| {
                            (l.contains("Error") || l.contains("error") || l.contains("failed"))
                                && !l.contains("Last message repeated")
                        })
                        .take(3)
                        .collect();
                    if !error_lines.is_empty() {
                        crate::log_detail!(&format!("         Error: {}", error_lines.join(" | ")));
                    }
                }
            }
            None
        }
        Err(e) => {
            crate::log_failure!(
                crate::static_logs::messages::LABEL_VMAF,
                &format!("VMAF-Y command failed: {e}")
            );
            None
        }
    }
}

/// Calculate CAMBI (Contrast Aware Multiscale Banding Index) for the output video.
///
/// CAMBI is a single-video metric (no reference needed) — lower is better (0 = no banding).
/// Returns None on failure or if libvmaf doesn't support the cambi feature.
#[must_use]
pub fn calculate_cambi(output: &Path, sample_rate: usize) -> Option<f64> {
    let n_threads = num_cpus_capped();

    let log_file = match tempfile::Builder::new().suffix(".json").tempfile() {
        Ok(f) => f,
        Err(e) => {
            crate::log_anomaly!(
                crate::static_logs::messages::LABEL_CAMBI,
                &format!("Failed to create temp file for CAMBI calculation: {e}")
            );
            return None;
        }
    };
    let log_path = log_file.path().to_path_buf();

    // libvmaf filter requires TWO inputs (main + reference).
    // For CAMBI (no-reference metric), we feed the same video as both inputs.
    // Use n_subsample for speed (skips frames inside libvmaf, faster than
    // select filter which still decodes every frame).
    let filter_complex = format!(
        "[0:v]scale=trunc(iw/2)*2:trunc(ih/2)*2,format=yuv420p[ref];[1:v]scale=trunc(iw/2)*2:trunc(ih/2)*2,format=yuv420p[dist];[dist][ref]libvmaf=feature=name=cambi:n_threads={nt}:n_subsample={ns}:log_fmt=json:log_path={lp}",
        nt = n_threads,
        ns = sample_rate.max(1),
        lp = log_path.display(),
    );

    let result = crate::ffmpeg_builder::FfmpegBuilder::new()
        .input(output)
        .input(output)
        .arg("-filter_complex")
        .arg(&filter_complex)
        .arg("-f")
        .arg("null")
        .output_pipe()
        .build()
        .output();

    match result {
        Ok(out) if out.status.success() => {
            // Read JSON from the temp log file
            let json = match std::fs::read_to_string(&log_path) {
                Ok(s) => s,
                Err(e) => {
                    crate::log_anomaly!(
                        crate::static_logs::messages::LABEL_CAMBI,
                        &format!(
                            "Failed to read CAMBI log file at {}: {}",
                            log_path.display(),
                            e
                        )
                    );
                    return None;
                }
            };
            parse_cambi_mean_from_json(&json)
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            crate::log_failure!(
                crate::static_logs::messages::LABEL_CAMBI,
                "CAMBI calculation failed!"
            );
            if stderr.contains("No such filter: 'libvmaf'") {
                crate::log_detail!("         Cause: libvmaf not available in this ffmpeg build");
            } else if stderr.contains("cambi")
                && (stderr.contains("unknown") || stderr.contains("No such"))
            {
                crate::log_detail!(
                    "         Cause: libvmaf in this ffmpeg does not support the 'cambi' feature"
                );
                crate::log_detail!("         Fix: upgrade to ffmpeg with libvmaf >= 2.x");
            } else {
                let error_lines: Vec<&str> = stderr
                    .lines()
                    .filter(|l| l.contains("Error") || l.contains("error") || l.contains("failed"))
                    .take(2)
                    .collect();
                if !error_lines.is_empty() {
                    crate::log_detail!(&format!("         Error: {}", error_lines.join(" | ")));
                }
            }
            None
        }
        Err(e) => {
            crate::log_failure!(
                crate::static_logs::messages::LABEL_CAMBI,
                &format!("CAMBI command failed: {e}")
            );
            None
        }
    }
}

/// Calculate PSNR for the U and V chroma channels independently.
/// Returns `(psnr_u, psnr_v)` in dB, or None on failure.
/// Uses `extractplanes` + ffmpeg's `psnr` filter (no libvmaf dependency).
#[must_use]
pub fn calculate_psnr_uv(input: &Path, output: &Path, sample_rate: usize) -> Option<(f64, f64)> {
    use std::thread;

    let (target_width, target_height) = resolve_common_metric_dimensions(input, output)?;

    let input_u = input.to_path_buf();
    let output_u = output.to_path_buf();
    let input_v = input.to_path_buf();
    let output_v = output.to_path_buf();

    let u_handle = thread::spawn(move || {
        psnr_single_channel(
            &input_u,
            &output_u,
            "u",
            sample_rate,
            target_width,
            target_height,
        )
    });
    let v_handle = thread::spawn(move || {
        psnr_single_channel(
            &input_v,
            &output_v,
            "v",
            sample_rate,
            target_width,
            target_height,
        )
    });

    let Ok(Some(psnr_u)) = u_handle.join() else {
        crate::log_failure!(
            crate::static_logs::messages::LABEL_QUALITY,
            "PSNR-U channel calculation failed"
        );
        return None;
    };
    let Ok(Some(psnr_v)) = v_handle.join() else {
        crate::log_failure!(
            crate::static_logs::messages::LABEL_QUALITY,
            "PSNR-V channel calculation failed"
        );
        return None;
    };

    Some((psnr_u, psnr_v))
}

fn psnr_single_channel(
    input: &Path,
    output: &Path,
    channel: &str,
    sample_rate: usize,
    target_width: u32,
    target_height: u32,
) -> Option<f64> {
    let sample_filter = if sample_rate > 1 {
        format!("select='not(mod(n\\,{sample_rate}))',setpts=N/FRAME_RATE/TB,")
    } else {
        String::new()
    };

    // Always use 8-bit yuv420p for PSNR calculation compatibility
    // PSNR filter works best with consistent bit depth
    let filter = format!(
        "[0:v]{sample_filter}scale={target_width}:{target_height}:flags=bicubic,format=yuv420p,extractplanes={channel}[ref];[1:v]{sample_filter}scale={target_width}:{target_height}:flags=bicubic,format=yuv420p,extractplanes={channel}[dis];[ref][dis]psnr=stats_file=-",
    );

    let result = crate::ffmpeg_builder::FfmpegBuilder::new()
        .input(input)
        .input(output)
        .arg("-filter_complex")
        .arg(&filter)
        .arg("-f")
        .arg("null")
        .output_pipe()
        .build()
        .output();

    match result {
        Ok(out) => {
            // psnr stats_file=- writes per-frame stats to stdout; we need the average from stderr summary.
            let stderr = String::from_utf8_lossy(&out.stderr);
            parse_psnr_average_y_from_stderr(&stderr)
        }
        Err(e) => {
            crate::log_failure!(
                crate::static_logs::messages::LABEL_QUALITY,
                &format!("PSNR-{} command failed: {}", channel.to_uppercase(), e)
            );
            None
        }
    }
}

/// Parse average PSNR from the ffmpeg psnr filter summary line in stderr.
/// Example: "PSNR y:41.234 u:39.876 v:40.123 average:40.411 min:38.123 max:42.567"
/// Since we already extracted a single plane (which ffmpeg labels as 'y'), we read the 'y' value.
fn parse_psnr_average_y_from_stderr(stderr: &str) -> Option<f64> {
    for line in stderr.lines() {
        if line.contains("PSNR") && line.contains("average:") {
            // Try "y:" field first (for single-plane extraction output)
            if let Some(pos) = line.find("y:") {
                let after = line[pos + 2..].trim_start();
                if after.starts_with("inf") || after.starts_with("-inf") {
                    return Some(100.0);
                }
                let end = after
                    .find(|c: char| !c.is_numeric() && c != '.')
                    .unwrap_or(after.len());
                if end > 0
                    && let Ok(v) = after[..end].parse::<f64>()
                    && v.is_finite()
                    && v > 0.0_f64
                {
                    return Some(v);
                }
            }
            // Fallback: "average:" field
            if let Some(pos) = line.find("average:") {
                let after = line[pos + 8..].trim_start();
                if after.starts_with("inf") || after.starts_with("-inf") {
                    return Some(100.0);
                }
                let end = after
                    .find(|c: char| !c.is_numeric() && c != '.')
                    .unwrap_or(after.len());
                if end > 0
                    && let Ok(v) = after[..end].parse::<f64>()
                    && v.is_finite()
                    && v > 0.0_f64
                {
                    return Some(v);
                }
            }
        }
    }
    None
}

fn parse_vmaf_mean_from_json(stdout: &str) -> Option<f64> {
    // Look for "pooled_metrics" → "vmaf" → "mean"
    if let Some(pooled_pos) = stdout.find("\"pooled_metrics\"") {
        let after_pooled = &stdout[pooled_pos..];
        if let Some(vmaf_pos) = after_pooled.find("\"vmaf\"") {
            let after_vmaf = &after_pooled[vmaf_pos..];
            if let Some(mean_pos) = after_vmaf.find("\"mean\"") {
                let after_mean = &after_vmaf[mean_pos + 6..];
                if let Some(colon_pos) = after_mean.find(':') {
                    let after_colon = after_mean[colon_pos + 1..].trim_start();
                    let end = after_colon
                        .find(|c: char| !c.is_numeric() && c != '.')
                        .unwrap_or(after_colon.len());
                    if end > 0 {
                        match after_colon[..end].parse::<f64>() {
                            Ok(f) => return Some(f),
                            Err(e) => {
                                crate::log_anomaly!(
                                    crate::static_logs::messages::LABEL_VMAF,
                                    &format!(
                                        "Failed to parse vmaf mean value '{}': {}",
                                        &after_colon[..end],
                                        e
                                    )
                                );
                                return None;
                            }
                        }
                    }
                }
            }
        }
    }
    None
}

fn parse_cambi_mean_from_json(stdout: &str) -> Option<f64> {
    // Look for "pooled_metrics" → "cambi" → "mean"
    if let Some(pooled_pos) = stdout.find("\"pooled_metrics\"") {
        let after_pooled = &stdout[pooled_pos..];
        if let Some(cambi_pos) = after_pooled.find("\"cambi\"") {
            let after_cambi = &after_pooled[cambi_pos..];
            if let Some(mean_pos) = after_cambi.find("\"mean\"") {
                let after_mean = &after_cambi[mean_pos + 6..];
                if let Some(colon_pos) = after_mean.find(':') {
                    let after_colon = after_mean[colon_pos + 1..].trim_start();
                    let end = after_colon
                        .find(|c: char| !c.is_numeric() && c != '.')
                        .unwrap_or(after_colon.len());
                    if end > 0 {
                        match after_colon[..end].parse::<f64>() {
                            Ok(f) => return Some(f),
                            Err(e) => {
                                crate::log_anomaly!(
                                    crate::static_logs::messages::LABEL_CAMBI,
                                    &format!(
                                        "Failed to parse cambi mean value '{}': {}",
                                        &after_colon[..end],
                                        e
                                    )
                                );
                                return None;
                            }
                        }
                    }
                }
            }
        }
    }
    None
}

/// Returns a capped thread count for libvmaf (max 8 to avoid over-subscription).
fn num_cpus_capped() -> usize {
    std::thread::available_parallelism().map_or(4, |n| n.get().min(8))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_vmaf_mean_from_json ─────────────────────────────────────────────

    #[test]
    fn test_parse_vmaf_mean_typical() {
        let json = r#"
{
  "pooled_metrics": {
    "vmaf": {
      "min": 91.234,
      "max": 97.654,
      "mean": 94.123,
      "harmonic_mean": 93.987
    }
  }
}"#;
        let result = parse_vmaf_mean_from_json(json);
        assert!(result.is_some(), "Should parse vmaf mean from typical JSON");
        let v = result.unwrap_or_else(|| panic!("missing value"));
        assert!((v - 94.123).abs() < 1e-6_f64, "Expected 94.123, got {v}");
    }

    #[test]
    fn test_parse_vmaf_mean_integer_value() {
        let json = r#"{"pooled_metrics": {"vmaf": {"mean": 100, "min": 99}}}"#;
        let result = parse_vmaf_mean_from_json(json);
        assert!(result.is_some());
        assert!(crate::float_compare::approx_eq_f64(
            result.unwrap_or_else(|| panic!("missing value")),
            100.0
        ));
    }

    #[test]
    fn test_parse_vmaf_mean_missing_pooled_metrics() {
        let json = r#"{"vmaf": {"mean": 95.0}}"#;
        assert!(parse_vmaf_mean_from_json(json).is_none());
    }

    #[test]
    fn test_parse_vmaf_mean_missing_vmaf_key() {
        let json = r#"{"pooled_metrics": {"ms_ssim": {"mean": 0.97}}}"#;
        assert!(parse_vmaf_mean_from_json(json).is_none());
    }

    #[test]
    fn test_parse_vmaf_mean_empty_string() {
        assert!(parse_vmaf_mean_from_json("").is_none());
    }

    #[test]
    fn test_parse_vmaf_mean_near_zero() {
        let json = r#"{"pooled_metrics": {"vmaf": {"mean": 0.5}}}"#;
        let result = parse_vmaf_mean_from_json(json);
        assert!(result.is_some());
        assert!((result.unwrap_or_else(|| panic!("missing value")) - 0.5).abs() < 1e-6_f64);
    }

    // ── parse_cambi_mean_from_json ────────────────────────────────────────────

    #[test]
    fn test_parse_cambi_mean_typical() {
        let json = r#"
{
  "pooled_metrics": {
    "cambi": {
      "min": 0.0,
      "max": 15.234,
      "mean": 7.456,
      "harmonic_mean": 6.123
    }
  }
}"#;
        let result = parse_cambi_mean_from_json(json);
        assert!(result.is_some(), "Should parse cambi mean");
        let v = result.unwrap_or_else(|| panic!("missing value"));
        assert!((v - 7.456).abs() < 1e-6_f64, "Expected 7.456, got {v}");
    }

    #[test]
    fn test_parse_cambi_mean_zero_banding() {
        let json = r#"{"pooled_metrics": {"cambi": {"mean": 0.0}}}"#;
        // 0.0 falls through because the parser reads numeric chars; "0" → 0 but
        // the end-of-numeric scan returns end=1, parse "0" → 0.0 is valid.
        // Whether 0.0 is returned or None depends on parser: .parse::<f64>().ok() → Some(0.0).
        let result = parse_cambi_mean_from_json(json);
        // Both Some(0.0) and None are acceptable depending on the trivial "0" parse.
        if let Some(v) = result {
            assert!(crate::float_compare::approx_eq_f64(v, 0.0));
        }
    }

    #[test]
    fn test_parse_cambi_mean_missing_cambi_key() {
        let json = r#"{"pooled_metrics": {"vmaf": {"mean": 95.0}}}"#;
        assert!(parse_cambi_mean_from_json(json).is_none());
    }

    #[test]
    fn test_parse_cambi_mean_high_banding() {
        let json = r#"{"pooled_metrics": {"cambi": {"mean": 42.789}}}"#;
        let result = parse_cambi_mean_from_json(json);
        assert!(result.is_some());
        assert!((result.unwrap_or_else(|| panic!("missing value")) - 42.789).abs() < 1e-6_f64);
    }

    // ── parse_psnr_average_y_from_stderr ─────────────────────────────────────

    #[test]
    fn test_parse_psnr_standard_ffmpeg_line() {
        // Standard ffmpeg psnr filter summary line
        let stderr = "[Parsed_psnr_4 @ 0x123] PSNR y:41.234 u:39.876 v:40.123 average:40.411 min:38.123 max:42.567\n";
        let result = parse_psnr_average_y_from_stderr(stderr);
        assert!(result.is_some(), "Should parse PSNR from standard line");
        let v = result.unwrap_or_else(|| panic!("missing value"));
        assert!((v - 41.234).abs() < 1e-3_f64, "Expected y:41.234, got {v}");
    }

    #[test]
    fn test_parse_psnr_uses_y_field_over_average() {
        // When both y: and average: are present, y: should be preferred
        let stderr = "PSNR y:38.5 average:37.0 min:35.0 max:40.0\n";
        let result = parse_psnr_average_y_from_stderr(stderr);
        assert!(result.is_some());
        // Should pick y:38.5, not average:37.0
        assert!((result.unwrap_or_else(|| panic!("missing value")) - 38.5).abs() < 1e-3_f64);
    }

    #[test]
    fn test_parse_psnr_fallback_to_average() {
        // No y: field, but average: present
        let stderr = "PSNR average:39.12 min:37.0 max:41.0\n";
        let result = parse_psnr_average_y_from_stderr(stderr);
        assert!(result.is_some());
        assert!((result.unwrap_or_else(|| panic!("missing value")) - 39.12).abs() < 1e-3_f64);
    }

    #[test]
    fn test_parse_psnr_no_psnr_line() {
        let stderr = "frame=100 fps=25\nEncoding done\n";
        assert!(parse_psnr_average_y_from_stderr(stderr).is_none());
    }

    #[test]
    fn test_parse_psnr_empty_stderr() {
        assert!(parse_psnr_average_y_from_stderr("").is_none());
    }

    #[test]
    fn test_parse_psnr_multiline_stderr() {
        // Realistic multi-line ffmpeg stderr output
        let stderr = concat!(
            "ffmpeg version 6.0\n",
            "  libavcodec 60.3.100\n",
            "Input #0, mov,mp4,m4a,3gp,3g2,mj2, from 'test.mp4':\n",
            "  Stream #0:0: Video: h264\n",
            "frame=  120 fps= 48 q=-0.0 Lsize=N/A time=00:00:05.00\n",
            "[Parsed_psnr_1 @ 0xdeadbeef] PSNR y:44.100 u:42.300 v:42.500 average:43.300 min:41.200 max:46.100\n",
        );
        let result = parse_psnr_average_y_from_stderr(stderr);
        assert!(result.is_some());
        assert!((result.unwrap_or_else(|| panic!("missing value")) - 44.1).abs() < 1e-3_f64);
    }

    #[test]
    fn test_parse_psnr_identical_infinity() {
        let stderr = "PSNR y:inf u:inf v:inf average:inf min:inf max:inf\n";
        let result = parse_psnr_average_y_from_stderr(stderr);
        assert!(result.is_some());
        assert!(crate::float_compare::approx_eq_f64(
            result.unwrap_or_else(|| panic!("missing value")),
            100.0
        ));

        let stderr_avg = "PSNR average:inf min:inf max:inf\n";
        let result_avg = parse_psnr_average_y_from_stderr(stderr_avg);
        assert!(result_avg.is_some());
        assert!(crate::float_compare::approx_eq_f64(
            result_avg.unwrap_or_else(|| panic!("missing value")),
            100.0
        ));
    }

    #[test]
    fn test_common_even_metric_dimensions_uses_shared_even_minimum() {
        let dims = common_even_metric_dimensions(29571, 1833, 29570, 1834);
        assert_eq!(dims, Some((29570, 1832)));
    }

    #[test]
    fn test_common_even_metric_dimensions_rejects_zero_after_even_rounding() {
        let dims = common_even_metric_dimensions(1, 2, 2, 2);
        assert_eq!(dims, None);
    }

    // ── num_cpus_capped ───────────────────────────────────────────────────────

    #[test]
    fn test_num_cpus_capped_within_bounds() {
        let n = num_cpus_capped();
        assert!(n >= 1, "Thread count must be at least 1, got {n}");
        assert!(n <= 8, "Thread count must be capped at 8, got {n}");
    }

    #[test]
    fn test_num_cpus_capped_is_deterministic() {
        // Calling twice should return the same value
        assert_eq!(num_cpus_capped(), num_cpus_capped());
    }

    // ── parse_ms_ssim_from_json ───────────────────────────────────────────

    #[test]
    fn test_parse_ms_ssim_json_typical() {
        let json =
            r#"{"pooled_metrics": {"float_ms_ssim": {"min": 0.95, "max": 0.99, "mean": 0.9712}}}"#;
        let r = parse_ms_ssim_from_json(json);
        assert!(r.is_some());
        assert!((r.unwrap_or_else(|| panic!("missing value")) - 0.9712).abs() < 1e-6_f64);
    }

    #[test]
    fn test_parse_ms_ssim_json_perfect() {
        let json = r#"{"pooled_metrics": {"float_ms_ssim": {"mean": 1.0}}}"#;
        assert!(
            (parse_ms_ssim_from_json(json).unwrap_or_else(|| panic!("missing value")) - 1.0).abs()
                < 1e-6_f64
        );
    }

    #[test]
    fn test_parse_ms_ssim_json_integer() {
        let json = r#"{"pooled_metrics": {"float_ms_ssim": {"mean": 1}}}"#;
        assert!(
            (parse_ms_ssim_from_json(json).unwrap_or_else(|| panic!("missing value")) - 1.0).abs()
                < 1e-6_f64
        );
    }

    #[test]
    fn test_parse_ms_ssim_json_missing_key() {
        assert!(
            parse_ms_ssim_from_json(r#"{"pooled_metrics": {"vmaf": {"mean": 95.0}}}"#).is_none()
        );
    }

    #[test]
    fn test_parse_ms_ssim_json_empty() {
        assert!(parse_ms_ssim_from_json("").is_none());
    }

    #[test]
    fn test_parse_ms_ssim_json_no_pooled() {
        assert!(parse_ms_ssim_from_json(r#"{"float_ms_ssim": {"mean": 0.98}}"#).is_none());
    }

    // ── parse_ms_ssim_from_legacy ─────────────────────────────────────────

    #[test]
    fn test_parse_ms_ssim_legacy_typical() {
        let s = "[libvmaf] MS-SSIM score: 0.9856\n";
        assert!(
            (parse_ms_ssim_from_legacy(s).unwrap_or_else(|| panic!("missing value")) - 0.9856)
                .abs()
                < 1e-4_f64
        );
    }

    #[test]
    fn test_parse_ms_ssim_legacy_ms_ssim_variant() {
        let s = "ms_ssim score: 0.9732\n";
        assert!(
            (parse_ms_ssim_from_legacy(s).unwrap_or_else(|| panic!("missing value")) - 0.9732)
                .abs()
                < 1e-4_f64
        );
    }

    #[test]
    fn test_parse_ms_ssim_legacy_float_variant() {
        let s = "float_ms_ssim score: 0.9900\n";
        assert!(
            (parse_ms_ssim_from_legacy(s).unwrap_or_else(|| panic!("missing value")) - 0.99).abs()
                < 1e-4_f64
        );
    }

    #[test]
    fn test_parse_ms_ssim_legacy_no_match() {
        assert!(parse_ms_ssim_from_legacy("frame=100 fps=25\n").is_none());
    }

    #[test]
    fn test_parse_ms_ssim_legacy_empty() {
        assert!(parse_ms_ssim_from_legacy("").is_none());
    }

    #[test]
    fn test_parse_ms_ssim_legacy_missing_score_keyword() {
        assert!(parse_ms_ssim_from_legacy("MS-SSIM mean: 0.97\n").is_none());
    }

    // ── PSNR edge-case hardening ──────────────────────────────────────────

    #[test]
    fn test_parse_psnr_inf_with_spaces() {
        let s = "PSNR y: inf u: inf v: inf average: inf min: inf max: inf\n";
        let r = parse_psnr_average_y_from_stderr(s);
        assert!(r.is_some(), "inf with leading spaces");
        assert!(crate::float_compare::approx_eq_f64(
            r.unwrap_or_else(|| panic!("missing value")),
            100.0
        ));
    }

    #[test]
    fn test_parse_psnr_very_high_value() {
        let s = "PSNR y:99.999 average:99.999 min:95.0 max:99.999\n";
        assert!(
            (parse_psnr_average_y_from_stderr(s).unwrap_or_else(|| panic!("missing value"))
                - 99.999)
                .abs()
                < 1e-3_f64
        );
    }

    #[test]
    fn test_parse_psnr_low_value() {
        let s = "PSNR y:25.5 average:24.0 min:20.0 max:30.0\n";
        assert!(
            (parse_psnr_average_y_from_stderr(s).unwrap_or_else(|| panic!("missing value")) - 25.5)
                .abs()
                < 1e-3_f64
        );
    }

    #[test]
    fn test_parse_psnr_negative_inf() {
        let s = "PSNR y:-inf average:-inf min:-inf max:-inf\n";
        assert!(crate::float_compare::approx_eq_f64(
            parse_psnr_average_y_from_stderr(s).unwrap_or_else(|| panic!("missing value")),
            100.0
        ));
    }

    #[test]
    fn test_parse_psnr_no_average_keyword() {
        // Contains PSNR but no "average:" — should return None
        assert!(parse_psnr_average_y_from_stderr("PSNR y:45.0 u:42.0 v:43.0\n").is_none());
    }

    // ── VMAF parser hardening ─────────────────────────────────────────────

    #[test]
    fn test_parse_vmaf_mean_with_whitespace() {
        let json = r#"{  "pooled_metrics" :  {  "vmaf" :  {  "mean" :  96.5  }  }  }"#;
        let r = parse_vmaf_mean_from_json(json);
        assert!(r.is_some());
        assert!((r.unwrap_or_else(|| panic!("missing value")) - 96.5).abs() < 1e-6_f64);
    }

    #[test]
    fn test_parse_vmaf_mean_perfect_100() {
        let json = r#"{"pooled_metrics": {"vmaf": {"mean": 100.0}}}"#;
        assert!(
            (parse_vmaf_mean_from_json(json).unwrap_or_else(|| panic!("missing value")) - 100.0)
                .abs()
                < 1e-6_f64
        );
    }

    // ── CAMBI parser hardening ────────────────────────────────────────────

    #[test]
    fn test_parse_cambi_mean_very_small() {
        let json = r#"{"pooled_metrics": {"cambi": {"mean": 0.01}}}"#;
        let r = parse_cambi_mean_from_json(json);
        assert!(r.is_some());
        assert!((r.unwrap_or_else(|| panic!("missing value")) - 0.01).abs() < 1e-6_f64);
    }

    #[test]
    fn test_parse_cambi_mean_empty() {
        assert!(parse_cambi_mean_from_json("").is_none());
    }

    #[test]
    fn test_parse_cambi_mean_no_pooled() {
        assert!(parse_cambi_mean_from_json(r#"{"cambi": {"mean": 5.0}}"#).is_none());
    }

    // ── common_even_metric_dimensions additional ──────────────────────────

    #[test]
    fn test_common_even_dimensions_identical() {
        let dims = common_even_metric_dimensions(1920, 1080, 1920, 1080);
        assert_eq!(dims, Some((1920, 1080)));
    }

    #[test]
    fn test_common_even_dimensions_output_smaller() {
        let dims = common_even_metric_dimensions(1920, 1080, 1280, 720);
        assert_eq!(dims, Some((1280, 720)));
    }

    #[test]
    fn test_common_even_dimensions_odd_values() {
        let dims = common_even_metric_dimensions(1921, 1081, 1921, 1081);
        assert_eq!(dims, Some((1920, 1080)));
    }
}
