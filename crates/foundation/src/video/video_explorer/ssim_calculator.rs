//! MS-SSIM quality metric calculations (multi-scale, YUV channel-wise)
//!
//! Primary entry: `calculate_ms_ssim_yuv` (used by `gpu_coarse_search` Phase
//! 3). `calculate_ms_ssim` is single-channel luma with standalone-vmaf fallback
//! for other callers.

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

fn resolve_common_metric_dimensions(input: &Path, output: &Path) -> anyhow::Result<(u32, u32)> {
    let (input_width, input_height) = crate::conversion::get_input_dimensions(input)
        .map_err(|err| anyhow::anyhow!("reference dimensions for quality metric: {err}"))?;
    let (output_width, output_height) = crate::conversion::get_input_dimensions(output)
        .map_err(|err| anyhow::anyhow!("distorted dimensions for quality metric: {err}"))?;

    common_even_metric_dimensions(input_width, input_height, output_width, output_height)
        .ok_or_else(|| anyhow::anyhow!("metric dimensions collapsed to zero after even rounding"))
}

/// `max_duration_min`: skip MS-SSIM when video longer than this (e.g. 5.0
/// normal, 25.0 ultimate).
///
/// Policy skips (GIF, over-duration limit) return `Ok(None)`. Probe or compute
/// failures return `Err`.
// Rationale: This function handles complex, sequential initialization or
// business logic where further fragmentation would hinder readability and
// maintainability.
pub fn calculate_ms_ssim_yuv(
    input: &Path,
    output: &Path,
    max_duration_min: f64,
) -> anyhow::Result<Option<(f64, f64, f64, f64)>> {
    use chrono::Local;
    use std::thread;

    if crate::image::format_detect::detect_true_format(input)?
        == crate::image::format_detect::FormatKind::Gif
    {
        crate::log_hint!(
            crate::infra::static_logs::messages::LABEL_MS_SSIM,
            "GIF format: skipping MS-SSIM (libvmaf incompatible), caller will use SSIM-All."
        );
        return Ok(None);
    }

    let duration = super::stream_analysis::get_video_duration(input)?;
    let duration_min = duration / 60.0_f64;

    // Caller sets max_duration_min (e.g. 5 min normal, 25 min ultimate) to control
    // skip threshold.
    let (sample_rate, should_calculate) =
        if duration_min <= crate::constants::QUALITY_ANALYSIS_SHORT_DURATION_MIN {
            (crate::constants::QUALITY_ANALYSIS_SAMPLE_RATE_SHORT, true)
        } else if duration_min <= max_duration_min {
            (crate::constants::QUALITY_ANALYSIS_SAMPLE_RATE_LONG, true)
        } else {
            (0, false)
        };

    if !should_calculate {
        crate::log_detail!(crate::infra::static_logs::messages::MSG_SSIM_FAST_MODE);
        return Ok(None);
    }

    let start_ts = Local::now().format("%Y-%m-%d %H:%M:%S");
    crate::log_detail!(crate::infra::static_logs::messages::MSG_MS_SSIM_INIT);
    crate::log_detail!(&format!("   Start time: {start_ts}"));
    crate::log_detail!(&format!(
        "   Input context: {duration:.1}s ({duration_min:.1}min) video stream"
    ));

    if sample_rate > 1 {
        let sample_rate_u32 = crate::numeric_cast::usize_to_u32_strict(sample_rate, "sample_rate")
            .ok_or_else(|| anyhow::anyhow!("sample_rate overflow for MS-SSIM"))?;
        let estimated_time = crate::numeric_cast::f64_to_u64_strict(
            duration / f64::from(sample_rate_u32) * 3.0,
            "estimated_time",
        )
        .ok_or_else(|| anyhow::anyhow!("estimated_time overflow for MS-SSIM"))?;
        crate::log_detail!(&format!(
            "   Temporal Sampling: 1/{sample_rate} frames active (Estimated forensic window: \
             {estimated_time}s)"
        ));
    } else {
        let estimated_time =
            crate::numeric_cast::f64_to_u64_strict(duration * 3.0, "estimated_time")
                .ok_or_else(|| anyhow::anyhow!("estimated_time overflow for MS-SSIM"))?;
        crate::log_detail!(&format!(
            "   Exhaustive Analysis: Processing all frames (Estimated forensic window: \
             {estimated_time}s)"
        ));
    }
    crate::log_detail!(crate::infra::static_logs::messages::MSG_SSIM_STRATEGY);

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
        anyhow::bail!("Y channel MS-SSIM calculation failed");
    };
    let u_ms_ssim = match u_handle.join() {
        Ok(Some(v)) => Some(v),
        _ => None,
    };
    let v_ms_ssim = match v_handle.join() {
        Ok(Some(v)) => Some(v),
        _ => None,
    };

    crate::log_stat!(
        crate::infra::static_logs::messages::LABEL_MS_SSIM_AUDIT,
        format!("Y-channel score: {:.4}", y_ms_ssim)
    );
    if let Some(u) = u_ms_ssim {
        crate::log_stat!(
            crate::infra::static_logs::messages::LABEL_CHROMA_AUDIT,
            format!("U-channel score: {:.4}", u)
        );
    } else {
        crate::log_detail!(crate::infra::static_logs::messages::MSG_SSIM_U_BYPASS);
    }
    if let Some(v) = v_ms_ssim {
        crate::log_stat!(
            crate::infra::static_logs::messages::LABEL_CHROMA_AUDIT,
            format!("V-channel score: {:.4}", v)
        );
    } else {
        crate::log_detail!(crate::infra::static_logs::messages::MSG_SSIM_V_BYPASS);
    }

    let elapsed = start_time.elapsed().as_secs();
    let end_time = Local::now().format("%Y-%m-%d %H:%M:%S");
    crate::log_detail!(&format!(
        "   MS-SSIM Structural Audit: Completed in {elapsed}s (End: {end_time} - Precision Level \
         3)"
    ));

    if u_ms_ssim.is_none() || v_ms_ssim.is_none() {
        crate::log_detail!(
            "   Forensic: Falling back to Y-only structural metric (chroma planes missing or \
             invalid)"
        );
    }

    let Some(sealed) =
        crate::video_explorer::precision::seal_ms_ssim_yuv_bundle(y_ms_ssim, u_ms_ssim, v_ms_ssim)
    else {
        anyhow::bail!(
            "MS-SSIM YUV bundle rejected (Y={y_ms_ssim:.6}, U={u_ms_ssim:?}, V={v_ms_ssim:?})"
        );
    };

    Ok(Some(sealed))
}

fn calculate_ms_ssim_channel_sampled(
    input: &Path,
    output: &Path,
    channel: &str,
    sample_rate: usize,
    target_width: u32,
    target_height: u32,
) -> Option<f64> {
    // For chroma channels (U/V) in YUV 4:2:0, the extracted plane is half the
    // luma resolution. libvmaf MS-SSIM performs multi-scale downsampling and
    // fails with "scale below 1x1" when the plane is too small.
    // Minimum safe luma resolution for chroma MS-SSIM: 256x256 (chroma = 128x128).
    if matches!(channel, "u" | "v")
        && (target_width < crate::constants::MS_SSIM_CHROMA_MIN_DIM
            || target_height < crate::constants::MS_SSIM_CHROMA_MIN_DIM)
    {
        crate::log_detail!(&format!(
            "      ℹ️  Channel {}: resolution {}x{} too small for chroma MS-SSIM (min {}x{}), \
             skipping",
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
        "[0:v]{sample_filter}scale={target_width}:{target_height}:flags=bicubic,format=yuv420p,\
         extractplanes={channel}[c0];[1:v]{sample_filter}scale={target_width}:{target_height}:\
         flags=bicubic,format=yuv420p,extractplanes={channel}[c1];[c0][c1]libvmaf=feature='\
         name=float_ms_ssim':log_fmt=json:log_path=/dev/stdout",
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
                if !out.status.success() {
                    // Score parsed from a run that did not exit cleanly: the pooled JSON may
                    // cover a truncated frame set. Accept but surface the condition.
                    let stderr_tail =
                        crate::io_utils::tail_error_lines(&String::from_utf8_lossy(&out.stderr), 3);
                    crate::media_conversion_gate::explore_ssim_metric_degraded_audit(
                        "explore_ssim_audit",
                        format!(
                            "Channel {} MS-SSIM score {score:.4} parsed despite ffmpeg exit {:?}; \
                             stderr tail: {stderr_tail}",
                            channel.to_uppercase(),
                            out.status.code()
                        ),
                    );
                }
                return Some(score);
            }

            // Only report failure if we truly got no usable result
            if !out.status.success() {
                let stderr = String::from_utf8_lossy(&out.stderr);
                crate::log_failure!(
                    crate::infra::static_logs::messages::LABEL_MS_SSIM,
                    &format!("Channel {} MS-SSIM failed!", channel.to_uppercase())
                );

                if stderr.contains("No such filter: 'libvmaf'") {
                    crate::log_detail!(crate::infra::static_logs::messages::MSG_SSIM_VMAF_MISSING);
                    crate::log_detail!(
                        "         Fix: brew install homebrew-ffmpeg/ffmpeg/ffmpeg --with-libvmaf"
                    );
                } else if stderr.contains("Invalid pixel format")
                    || stderr.contains("Discarding mismatched")
                {
                    crate::log_detail!(crate::infra::static_logs::messages::MSG_SSIM_PIX_FMT_ERR);
                    crate::log_detail!(
                        &crate::infra::static_logs::messages::MSG_SSIM_INPUT_DISPLAY
                            .replace("{}", &input.display().to_string())
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
                        crate::log_detail!(
                            &crate::infra::static_logs::messages::MSG_SSIM_ERR_LINES
                                .replace("{}", &error_lines.join(" | "))
                        );
                    }
                }
            }
            None
        }
        Err(e) => {
            crate::log_failure!(
                crate::infra::static_logs::messages::LABEL_MS_SSIM,
                &format!("Channel {} command failed: {e}", channel.to_uppercase())
            );
            None
        }
    }
}

pub fn calculate_ms_ssim(input: &Path, output: &Path) -> anyhow::Result<Option<f64>> {
    match crate::ffprobe::probe_video(input) {
        Ok(info) if info.width < 64 || info.height < 64 => return Ok(None),
        Ok(_) => {}
        Err(err) => {
            crate::media_conversion_gate::explore_ssim_metric_degraded_audit(
                "explore_ssim_audit",
                format!(
                    "Failed to probe input for single-channel MS-SSIM {}: {err}",
                    input.display()
                ),
            );
            return Err(anyhow::anyhow!(
                "Failed to probe input for single-channel MS-SSIM {}: {err}",
                input.display()
            ));
        }
    }

    crate::log_detail!(
        "Initiating single-channel MS-SSIM (Multi-Scale Structural Similarity) audit..."
    );

    let (target_width, target_height) = resolve_common_metric_dimensions(input, output)?;

    // Always use 8-bit yuv420p for libvmaf compatibility
    // libvmaf's MS-SSIM feature works best with 8-bit input
    let result = crate::ffmpeg_builder::FfmpegBuilder::new()
        .input(input)
        .input(output)
        .arg("-filter_complex")
        .arg(format!(
            "[0:v]scale={target_width}:{target_height}:flags=bicubic,format=yuv420p[ref];[1:\
             v]scale={target_width}:{target_height}:flags=bicubic,format=yuv420p[dis];\
             [ref][dis]libvmaf=log_path=/dev/stdout:log_fmt=json:feature='name=float_ms_ssim'",
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
                if !out.status.success() {
                    // Pooled score from an unclean exit may cover a truncated frame set.
                    let stderr_tail =
                        crate::io_utils::tail_error_lines(&String::from_utf8_lossy(&out.stderr), 3);
                    crate::media_conversion_gate::explore_ssim_metric_degraded_audit(
                        "explore_ssim_audit",
                        format!(
                            "MS-SSIM score {ms_ssim:.4} parsed despite ffmpeg exit {:?}; stderr \
                             tail: {stderr_tail}",
                            out.status.code()
                        ),
                    );
                }
                crate::log_stat!(
                    crate::infra::static_logs::messages::LABEL_MS_SSIM_AUDIT,
                    format!("Composite score: {ms_ssim:.4}")
                );
                return Ok(Some(ms_ssim));
            }

            if let Some(ms_ssim) = parse_ms_ssim_from_legacy(&stderr) {
                crate::log_detail!(
                    &crate::infra::static_logs::messages::MSG_SSIM_SCORE
                        .replace("{}", &format!("{ms_ssim:.4}"))
                );
                return Ok(Some(ms_ssim));
            }

            // No parseable score found
            if out.status.success() {
                crate::media_conversion_gate::explore_ssim_metric_degraded_audit(
                    "explore_ssim_audit",
                    "MS-SSIM calculated but failed to parse score",
                );
            } else {
                crate::log_failure!(
                    crate::infra::static_logs::messages::LABEL_MS_SSIM,
                    "ffmpeg libvmaf MS-SSIM failed"
                );
                crate::log_detail!(
                    "   Forensic: Primary metric failed; attempting standalone VMAF-bin \
                     structural recovery..."
                );

                if crate::vmaf_standalone::is_vmaf_available() {
                    match crate::vmaf_standalone::calculate_ms_ssim_standalone(input, output) {
                        Ok(score) => {
                            crate::log_detail!(
                                &crate::infra::static_logs::messages::MSG_SSIM_VMAF_SCORE
                                    .replace("{}", &format!("{score:.4}"))
                            );
                            return Ok(Some(score));
                        }
                        Err(e) => {
                            crate::media_conversion_gate::explore_ssim_metric_degraded_audit(
                                "explore_ssim_audit",
                                format!("Standalone vmaf also failed: {e}"),
                            );
                        }
                    }
                }
            }
        }
        Err(e) => {
            crate::log_failure!(
                crate::infra::static_logs::messages::LABEL_MS_SSIM,
                &format!("ffmpeg MS-SSIM failed: {e}")
            );
            return Err(anyhow::anyhow!(
                "failed to launch single-channel MS-SSIM for {} -> {}: {e}",
                input.display(),
                output.display()
            ));
        }
    }

    Ok(None)
}

fn pooled_metric_mean_from_json(stdout: &str, metric: &str) -> anyhow::Result<Option<f64>> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let root: serde_json::Value = serde_json::from_str(trimmed)
        .map_err(|err| anyhow::anyhow!("failed to parse libvmaf JSON: {err}"))?;
    let Some(mean) = root
        .get("pooled_metrics")
        .and_then(|pooled| pooled.get(metric))
        .and_then(|metric_value| metric_value.get("mean"))
    else {
        return Ok(None);
    };
    mean.as_f64().map(Some).ok_or_else(|| {
        anyhow::anyhow!("libvmaf pooled metric {metric}.mean must be a JSON number")
    })
}

fn parse_ms_ssim_from_json(stdout: &str) -> Option<f64> {
    let value = match pooled_metric_mean_from_json(stdout, "float_ms_ssim") {
        Ok(Some(value)) => value,
        Ok(None) => return None,
        Err(err) => {
            crate::media_conversion_gate::explore_ssim_metric_degraded_audit(
                "explore_ssim_audit",
                format!("Failed to parse float_ms_ssim JSON: {err}"),
            );
            return None;
        }
    };
    if let Some(sealed) = crate::video_explorer::precision::seal_ms_ssim(value) {
        return Some(sealed);
    }
    crate::media_conversion_gate::explore_metric_parse_reject_audit(
        "ms_ssim",
        format!("float_ms_ssim mean {value:.6} out of [0,1] domain"),
    );
    None
}

fn parse_ms_ssim_from_legacy(stderr: &str) -> Option<f64> {
    for line in stderr.lines() {
        if (line.contains("MS-SSIM") || line.contains("ms_ssim") || line.contains("float_ms_ssim"))
            && line.contains("score:")
            && let Some(score_pos) = line.find("score:")
        {
            let after_score = &line[score_pos + 6..].trim_start();
            let end = crate::media_conversion_gate::explore_metric_numeric_end(after_score, false);
            if end > 0 {
                match after_score[..end].parse::<f64>() {
                    Ok(f) => {
                        if let Some(sealed) = crate::video_explorer::precision::seal_ms_ssim(f) {
                            return Some(sealed);
                        }
                        crate::media_conversion_gate::explore_metric_parse_reject_audit(
                            "ms_ssim",
                            format!("legacy score {f:.6} out of [0,1] domain"),
                        );
                        return None;
                    }
                    Err(e) => {
                        crate::media_conversion_gate::explore_ssim_metric_degraded_audit(
                            "explore_ssim_audit",
                            format!(
                                "Failed to parse legacy ms_ssim value '{}': {}",
                                &after_score[..end],
                                e
                            ),
                        );
                        return None;
                    }
                }
            }
        }
    }
    None
}

// ─── Ultimate Mode: 3D Quality Metrics
// ────────────────────────────────────────

/// Calculate VMAF Y-channel score (perceptual quality, 0–100 scale).
/// `sample_rate`: 1 = every frame, 3 = every 3rd frame, etc.
/// Returns `Ok(None)` when no metric is present in successful output.
///
/// # Errors
/// Returns an error when probes, command execution, or metric parsing fails.
pub fn calculate_vmaf_y(
    input: &Path,
    output: &Path,
    sample_rate: usize,
) -> anyhow::Result<Option<f64>> {
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
        "[0:v]{sample_filter}scale={target_width}:{target_height}:flags=bicubic,\
         format=yuv420p[dis];[1:v]{sample_filter}scale={target_width}:{target_height}:\
         flags=bicubic,format=yuv420p[ref];[dis][ref]libvmaf=shortest=true:ts_sync_mode=nearest:\
         n_threads={n_threads}:log_fmt=json:log_path=/dev/stdout",
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
            if let Some(score) = parse_vmaf_mean_from_json(&stdout)? {
                if !out.status.success() {
                    // Pooled VMAF from an unclean exit may cover a truncated frame set.
                    let stderr_tail =
                        crate::io_utils::tail_error_lines(&String::from_utf8_lossy(&out.stderr), 3);
                    crate::media_conversion_gate::explore_ssim_metric_degraded_audit(
                        "explore_vmaf_audit",
                        format!(
                            "VMAF-Y score {score:.4} parsed despite ffmpeg exit {:?}; stderr \
                             tail: {stderr_tail}",
                            out.status.code()
                        ),
                    );
                }
                return Ok(Some(score));
            }

            if !out.status.success() {
                let stderr = String::from_utf8_lossy(&out.stderr);
                crate::log_failure!(
                    crate::infra::static_logs::messages::LABEL_VMAF_AUDIT,
                    "VMAF-Y calculation failed! (Forensic Analysis Aborted)"
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
                        crate::log_detail!(
                            &crate::infra::static_logs::messages::MSG_SSIM_ERR_LINES
                                .replace("{}", &error_lines.join(" | "))
                        );
                    }
                }
                anyhow::bail!("VMAF-Y calculation failed: {}", stderr.trim());
            }
            Ok(None)
        }
        Err(e) => {
            crate::log_failure!(
                crate::infra::static_logs::messages::LABEL_VMAF,
                &format!("VMAF-Y command failed: {e}")
            );
            Err(e).map_err(|err| anyhow::anyhow!("VMAF-Y command failed: {err}"))
        }
    }
}

/// Calculate CAMBI (Contrast Aware Multiscale Banding Index) for the output
/// video.
///
/// CAMBI is a single-video metric (no reference needed) — lower is better (0 =
/// no banding). Returns `Ok(None)` when no metric is present in successful
/// output.
///
/// # Errors
/// Returns an error when temp-file creation, command execution, log read, or
/// parsing fails.
pub fn calculate_cambi(output: &Path, sample_rate: usize) -> anyhow::Result<Option<f64>> {
    let n_threads = num_cpus_capped();

    let log_file = match crate::media_conversion_gate::delivery_named_tempfile_in_scratch_or_err(
        "explore_cambi_json",
        None,
        Some(".json"),
    ) {
        Ok(f) => f,
        Err(e) => {
            crate::media_conversion_gate::explore_ssim_metric_degraded_audit(
                "explore_audit",
                format!("Failed to create temp file for CAMBI calculation: {e}"),
            );
            anyhow::bail!("Failed to create temp file for CAMBI calculation: {e}");
        }
    };
    let log_path = log_file.path().to_path_buf();

    // libvmaf filter requires TWO inputs (main + reference).
    // For CAMBI (no-reference metric), we feed the same video as both inputs.
    // Use n_subsample for speed (skips frames inside libvmaf, faster than
    // select filter which still decodes every frame).
    let filter_complex = format!(
        "[0:v]scale=trunc(iw/2)*2:trunc(ih/2)*2,format=yuv420p[ref];[1:v]scale=trunc(iw/2)*2:\
         trunc(ih/2)*2,format=yuv420p[dist];[dist][ref]libvmaf=feature=name=cambi:n_threads={nt}:\
         n_subsample={ns}:log_fmt=json:log_path={lp}",
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
                    crate::media_conversion_gate::explore_ssim_metric_degraded_audit(
                        "explore_audit",
                        format!(
                            "Failed to read CAMBI log file at {}: {}",
                            log_path.display(),
                            e
                        ),
                    );
                    anyhow::bail!(
                        "Failed to read CAMBI log file at {}: {e}",
                        log_path.display()
                    );
                }
            };
            parse_cambi_mean_from_json(&json)
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            crate::log_failure!(
                crate::infra::static_logs::messages::LABEL_CAMBI_AUDIT,
                "CAMBI calculation failed! (Forensic Analysis Aborted)"
            );
            if stderr.contains("No such filter: 'libvmaf'") {
                crate::log_detail!(crate::infra::static_logs::messages::MSG_SSIM_VMAF_OLD);
                crate::log_detail!(crate::infra::static_logs::messages::MSG_SSIM_VMAF_FIX);
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
            anyhow::bail!("CAMBI calculation failed: {}", stderr.trim());
        }
        Err(e) => {
            crate::log_failure!(
                crate::infra::static_logs::messages::LABEL_CAMBI,
                &format!("CAMBI command failed: {e}")
            );
            Err(e).map_err(|err| anyhow::anyhow!("CAMBI command failed: {err}"))
        }
    }
}

/// Calculate PSNR for the U and V chroma channels independently.
///
/// Returns `(psnr_u, psnr_v)` in dB, or `Ok(None)` when successful output has
/// no metric. Uses `extractplanes` + ffmpeg's `psnr` filter (no libvmaf
/// dependency).
///
/// # Errors
/// Returns an error when probes, command execution, worker joins, or parsing
/// fails.
pub fn calculate_psnr_uv(
    input: &Path,
    output: &Path,
    sample_rate: usize,
) -> anyhow::Result<Option<(f64, f64)>> {
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

    let psnr_u = match u_handle.join() {
        Ok(Ok(Some(value))) => value,
        Ok(Ok(None)) => {
            crate::log_failure!(
                crate::infra::static_logs::messages::LABEL_QUALITY,
                "PSNR-U channel calculation produced no metric"
            );
            return Ok(None);
        }
        Ok(Err(err)) => anyhow::bail!("PSNR-U channel calculation failed: {err}"),
        Err(_) => anyhow::bail!("PSNR-U channel worker panicked"),
    };
    let psnr_v = match v_handle.join() {
        Ok(Ok(Some(value))) => value,
        Ok(Ok(None)) => {
            crate::log_failure!(
                crate::infra::static_logs::messages::LABEL_QUALITY,
                "PSNR-V channel calculation produced no metric"
            );
            return Ok(None);
        }
        Ok(Err(err)) => anyhow::bail!("PSNR-V channel calculation failed: {err}"),
        Err(_) => anyhow::bail!("PSNR-V channel worker panicked"),
    };

    Ok(Some((psnr_u, psnr_v)))
}

fn psnr_single_channel(
    input: &Path,
    output: &Path,
    channel: &str,
    sample_rate: usize,
    target_width: u32,
    target_height: u32,
) -> anyhow::Result<Option<f64>> {
    let sample_filter = if sample_rate > 1 {
        format!("select='not(mod(n\\,{sample_rate}))',setpts=N/FRAME_RATE/TB,")
    } else {
        String::new()
    };

    // Always use 8-bit yuv420p for PSNR calculation compatibility
    // PSNR filter works best with consistent bit depth
    let filter = format!(
        "[0:v]{sample_filter}scale={target_width}:{target_height}:flags=bicubic,format=yuv420p,\
         extractplanes={channel}[ref];[1:v]{sample_filter}scale={target_width}:{target_height}:\
         flags=bicubic,format=yuv420p,extractplanes={channel}[dis];[ref][dis]psnr=stats_file=-",
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
            // psnr stats_file=- writes per-frame stats to stdout; we need the average from
            // stderr summary.
            let stderr = String::from_utf8_lossy(&out.stderr);
            parse_psnr_average_y_from_stderr(&stderr)
        }
        Err(e) => {
            crate::log_failure!(
                crate::infra::static_logs::messages::LABEL_QUALITY,
                &format!("PSNR-{} command failed: {}", channel.to_uppercase(), e)
            );
            Err(e).map_err(|err| anyhow::anyhow!("PSNR-{channel} command failed: {err}"))
        }
    }
}

/// Parse average PSNR from the ffmpeg psnr filter summary line in stderr.
/// Example: "PSNR y:41.234 u:39.876 v:40.123 average:40.411 min:38.123
/// max:42.567" Since we already extracted a single plane (which ffmpeg labels
/// as 'y'), we read the 'y' value.
fn parse_psnr_average_y_from_stderr(stderr: &str) -> anyhow::Result<Option<f64>> {
    for line in stderr.lines() {
        if line.contains("PSNR") && line.contains("average:") {
            // Try "y:" field first (for single-plane extraction output)
            if let Some(pos) = line.find("y:")
                && let Some(sealed) =
                    crate::video_explorer::precision::parse_explore_psnr_metric_token(
                        &line[pos + 2..],
                    )
                    .map_err(|err| anyhow::anyhow!("failed to parse PSNR-Y metric token: {err}"))?
            {
                return Ok(Some(sealed));
            }
            // Fallback: "average:" field
            if let Some(pos) = line.find("average:")
                && let Some(sealed) =
                    crate::video_explorer::precision::parse_explore_psnr_metric_token(
                        &line[pos + 8..],
                    )
                    .map_err(|err| {
                        anyhow::anyhow!("failed to parse PSNR average metric token: {err}")
                    })?
            {
                return Ok(Some(sealed));
            }
        }
    }
    Ok(None)
}

fn parse_vmaf_mean_from_json(stdout: &str) -> anyhow::Result<Option<f64>> {
    let Some(value) = pooled_metric_mean_from_json(stdout, "vmaf")? else {
        return Ok(None);
    };
    if let Some(sealed) = crate::video_explorer::precision::seal_vmaf_y(value) {
        return Ok(Some(sealed));
    }
    crate::media_conversion_gate::explore_metric_parse_reject_audit(
        "vmaf_y",
        format!("mean value {value:.6} out of [0,100] domain"),
    );
    Ok(None)
}

fn parse_cambi_mean_from_json(stdout: &str) -> anyhow::Result<Option<f64>> {
    let Some(value) = pooled_metric_mean_from_json(stdout, "cambi")? else {
        return Ok(None);
    };
    if let Some(sealed) = crate::video_explorer::precision::seal_cambi(value) {
        return Ok(Some(sealed));
    }
    crate::media_conversion_gate::explore_metric_parse_reject_audit(
        "cambi",
        format!("mean value {value:.6} non-finite/negative"),
    );
    Ok(None)
}

/// Returns a capped thread count for libvmaf (max 8 to avoid
/// over-subscription).
fn num_cpus_capped() -> usize {
    crate::media_conversion_gate::runtime_available_parallelism_capped_or_default(
        8,
        "ssim_calculator::num_cpus_capped",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn ok_metric(result: anyhow::Result<Option<f64>>) -> Option<f64> {
        result.unwrap_or_else(|err| panic!("metric parse failed: {err}"))
    }

    fn metric_value(result: anyhow::Result<Option<f64>>) -> f64 {
        ok_metric(result).unwrap_or_else(|| panic!("missing value"))
    }

    #[test]
    fn ms_ssim_gif_skip_uses_content_not_suffix() {
        let temp = tempfile::tempdir().expect("create temp directory");
        let input = temp.path().join("animation.mp4");
        std::fs::write(&input, b"GIF89a\x01\x00\x01\x00").expect("write GIF signature");

        let result = calculate_ms_ssim_yuv(&input, &temp.path().join("missing.mp4"), 5.0)
            .expect("GIF policy skip should not invoke video probes");
        assert!(result.is_none());
    }

    #[test]
    fn common_even_metric_dimensions_rejects_zero_target() {
        assert!(common_even_metric_dimensions(0, 720, 1280, 720).is_none());
        assert!(common_even_metric_dimensions(1280, 0, 1280, 720).is_none());
    }

    #[test]
    fn resolve_common_metric_dimensions_errors_on_missing_files() {
        let missing = Path::new("/nonexistent/mfb_phase33_ms_ssim_input.mp4");
        let err = resolve_common_metric_dimensions(missing, missing)
            .expect_err("missing media must fail closed");
        assert!(
            err.to_string().contains("dimensions"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn calculate_ms_ssim_missing_input_returns_error_not_none() {
        let missing = Path::new("/nonexistent/mfb_ms_ssim_missing_input.mp4");

        let err = calculate_ms_ssim(missing, missing)
            .expect_err("missing MS-SSIM input must be an error");

        assert!(err.to_string().contains("mfb_ms_ssim_missing_input.mp4"));
    }

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
        let result = ok_metric(parse_vmaf_mean_from_json(json));
        assert!(result.is_some(), "Should parse vmaf mean from typical JSON");
        let v = result.unwrap_or_else(|| panic!("missing value"));
        assert!((v - 94.123).abs() < 1e-6_f64, "Expected 94.123, got {v}");
    }

    #[test]
    fn test_parse_vmaf_mean_integer_value() {
        let json = r#"{"pooled_metrics": {"vmaf": {"mean": 100, "min": 99}}}"#;
        let result = ok_metric(parse_vmaf_mean_from_json(json));
        assert!(result.is_some());
        assert!(crate::float_compare::approx_eq_f64(
            result.unwrap_or_else(|| panic!("missing value")),
            100.0
        ));
    }

    #[test]
    fn test_parse_vmaf_mean_missing_pooled_metrics() {
        let json = r#"{"vmaf": {"mean": 95.0}}"#;
        assert!(ok_metric(parse_vmaf_mean_from_json(json)).is_none());
    }

    #[test]
    fn test_parse_vmaf_mean_missing_vmaf_key() {
        let json = r#"{"pooled_metrics": {"ms_ssim": {"mean": 0.97}}}"#;
        assert!(ok_metric(parse_vmaf_mean_from_json(json)).is_none());
    }

    #[test]
    fn test_parse_vmaf_mean_empty_string() {
        assert!(ok_metric(parse_vmaf_mean_from_json("")).is_none());
    }

    #[test]
    fn test_parse_vmaf_mean_near_zero() {
        let json = r#"{"pooled_metrics": {"vmaf": {"mean": 0.5}}}"#;
        let result = ok_metric(parse_vmaf_mean_from_json(json));
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
        let result = ok_metric(parse_cambi_mean_from_json(json));
        assert!(result.is_some(), "Should parse cambi mean");
        let v = result.unwrap_or_else(|| panic!("missing value"));
        assert!((v - 7.456).abs() < 1e-6_f64, "Expected 7.456, got {v}");
    }

    #[test]
    fn test_parse_cambi_mean_zero_banding() {
        let json = r#"{"pooled_metrics": {"cambi": {"mean": 0.0}}}"#;
        assert!(crate::float_compare::approx_eq_f64(
            metric_value(parse_cambi_mean_from_json(json)),
            0.0
        ));
    }

    #[test]
    fn test_parse_cambi_mean_missing_cambi_key() {
        let json = r#"{"pooled_metrics": {"vmaf": {"mean": 95.0}}}"#;
        assert!(ok_metric(parse_cambi_mean_from_json(json)).is_none());
    }

    #[test]
    fn test_parse_cambi_mean_high_banding() {
        let json = r#"{"pooled_metrics": {"cambi": {"mean": 42.789}}}"#;
        let result = ok_metric(parse_cambi_mean_from_json(json));
        assert!(result.is_some());
        assert!((result.unwrap_or_else(|| panic!("missing value")) - 42.789).abs() < 1e-6_f64);
    }

    #[test]
    fn test_parse_cambi_mean_rejects_negative() {
        let json = r#"{"pooled_metrics": {"cambi": {"mean": -0.5}}}"#;
        assert!(ok_metric(parse_cambi_mean_from_json(json)).is_none());
    }

    // ── parse_psnr_average_y_from_stderr ─────────────────────────────────────

    #[test]
    fn test_parse_psnr_standard_ffmpeg_line() {
        // Standard ffmpeg psnr filter summary line
        let stderr = "[Parsed_psnr_4 @ 0x123] PSNR y:41.234 u:39.876 v:40.123 average:40.411 \
                      min:38.123 max:42.567\n";
        let result = ok_metric(parse_psnr_average_y_from_stderr(stderr));
        assert!(result.is_some(), "Should parse PSNR from standard line");
        let v = result.unwrap_or_else(|| panic!("missing value"));
        assert!((v - 41.234).abs() < 1e-3_f64, "Expected y:41.234, got {v}");
    }

    #[test]
    fn test_parse_psnr_uses_y_field_over_average() {
        // When both y: and average: are present, y: should be preferred
        let stderr = "PSNR y:38.5 average:37.0 min:35.0 max:40.0\n";
        let result = ok_metric(parse_psnr_average_y_from_stderr(stderr));
        assert!(result.is_some());
        // Should pick y:38.5, not average:37.0
        assert!((result.unwrap_or_else(|| panic!("missing value")) - 38.5).abs() < 1e-3_f64);
    }

    #[test]
    fn test_parse_psnr_fallback_to_average() {
        // No y: field, but average: present
        let stderr = "PSNR average:39.12 min:37.0 max:41.0\n";
        let result = ok_metric(parse_psnr_average_y_from_stderr(stderr));
        assert!(result.is_some());
        assert!((result.unwrap_or_else(|| panic!("missing value")) - 39.12).abs() < 1e-3_f64);
    }

    #[test]
    fn test_parse_psnr_no_psnr_line() {
        let stderr = "frame=100 fps=25\nEncoding done\n";
        assert!(ok_metric(parse_psnr_average_y_from_stderr(stderr)).is_none());
    }

    #[test]
    fn test_parse_psnr_empty_stderr() {
        assert!(ok_metric(parse_psnr_average_y_from_stderr("")).is_none());
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
            "[Parsed_psnr_1 @ 0xdeadbeef] PSNR y:44.100 u:42.300 v:42.500 average:43.300 \
             min:41.200 max:46.100\n",
        );
        let result = ok_metric(parse_psnr_average_y_from_stderr(stderr));
        assert!(result.is_some());
        assert!((result.unwrap_or_else(|| panic!("missing value")) - 44.1).abs() < 1e-3_f64);
    }

    #[test]
    fn test_parse_psnr_identical_infinity() {
        let stderr = "PSNR y:inf u:inf v:inf average:inf min:inf max:inf\n";
        let result = ok_metric(parse_psnr_average_y_from_stderr(stderr));
        assert!(result.is_some());
        assert!(crate::float_compare::approx_eq_f64(
            result.unwrap_or_else(|| panic!("missing value")),
            100.0
        ));

        let stderr_avg = "PSNR average:inf min:inf max:inf\n";
        let result_avg = ok_metric(parse_psnr_average_y_from_stderr(stderr_avg));
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

    #[test]
    fn test_parse_ms_ssim_json_rejects_out_of_range() {
        let too_high = r#"{"pooled_metrics": {"float_ms_ssim": {"mean": 1.2}}}"#;
        let negative = r#"{"pooled_metrics": {"float_ms_ssim": {"mean": -0.1}}}"#;
        assert!(parse_ms_ssim_from_json(too_high).is_none());
        assert!(parse_ms_ssim_from_json(negative).is_none());
    }

    #[test]
    fn pooled_metric_parsers_do_not_borrow_a_sibling_mean() {
        let json = r#"{
            "pooled_metrics": {
                "float_ms_ssim": {"min": 0.9},
                "vmaf": {"min": 90.0},
                "cambi": {"min": 0.0},
                "other": {"mean": 0.5}
            }
        }"#;

        assert!(parse_ms_ssim_from_json(json).is_none());
        assert!(ok_metric(parse_vmaf_mean_from_json(json)).is_none());
        assert!(ok_metric(parse_cambi_mean_from_json(json)).is_none());
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
        let r = ok_metric(parse_psnr_average_y_from_stderr(s));
        assert!(r.is_some(), "inf with leading spaces");
        assert!(crate::float_compare::approx_eq_f64(
            r.unwrap_or_else(|| panic!("missing value")),
            100.0
        ));
    }

    #[test]
    fn test_parse_psnr_very_high_value() {
        let s = "PSNR y:99.999 average:99.999 min:95.0 max:99.999\n";
        assert!((metric_value(parse_psnr_average_y_from_stderr(s)) - 99.999).abs() < 1e-3_f64);
    }

    #[test]
    fn test_parse_psnr_low_value() {
        let s = "PSNR y:25.5 average:24.0 min:20.0 max:30.0\n";
        assert!((metric_value(parse_psnr_average_y_from_stderr(s)) - 25.5).abs() < 1e-3_f64);
    }

    #[test]
    fn test_parse_psnr_negative_inf() {
        let s = "PSNR y:-inf average:-inf min:-inf max:-inf\n";
        assert!(crate::float_compare::approx_eq_f64(
            metric_value(parse_psnr_average_y_from_stderr(s)),
            100.0
        ));
    }

    #[test]
    fn test_parse_psnr_no_average_keyword() {
        // Contains PSNR but no "average:" — should return None
        assert!(
            ok_metric(parse_psnr_average_y_from_stderr(
                "PSNR y:45.0 u:42.0 v:43.0\n"
            ))
            .is_none()
        );
    }

    // ── VMAF parser hardening ─────────────────────────────────────────────

    #[test]
    fn test_parse_vmaf_mean_with_whitespace() {
        let json = r#"{  "pooled_metrics" :  {  "vmaf" :  {  "mean" :  96.5  }  }  }"#;
        let r = ok_metric(parse_vmaf_mean_from_json(json));
        assert!(r.is_some());
        assert!((r.unwrap_or_else(|| panic!("missing value")) - 96.5).abs() < 1e-6_f64);
    }

    #[test]
    fn test_parse_vmaf_mean_perfect_100() {
        let json = r#"{"pooled_metrics": {"vmaf": {"mean": 100.0}}}"#;
        assert!((metric_value(parse_vmaf_mean_from_json(json)) - 100.0).abs() < 1e-6_f64);
    }

    // ── CAMBI parser hardening ────────────────────────────────────────────

    #[test]
    fn test_parse_cambi_mean_very_small() {
        let json = r#"{"pooled_metrics": {"cambi": {"mean": 0.01}}}"#;
        let r = ok_metric(parse_cambi_mean_from_json(json));
        assert!(r.is_some());
        assert!((r.unwrap_or_else(|| panic!("missing value")) - 0.01).abs() < 1e-6_f64);
    }

    #[test]
    fn test_parse_cambi_mean_empty() {
        assert!(ok_metric(parse_cambi_mean_from_json("")).is_none());
    }

    #[test]
    fn test_parse_cambi_mean_no_pooled() {
        assert!(ok_metric(parse_cambi_mean_from_json(r#"{"cambi": {"mean": 5.0}}"#)).is_none());
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
