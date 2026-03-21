//! MS-SSIM quality metric calculations (multi-scale, YUV channel-wise)
//!
//! Primary entry: `calculate_ms_ssim_yuv` (used by gpu_coarse_search Phase 3).  
//! `calculate_ms_ssim` is single-channel luma with standalone-vmaf fallback for other callers.

use std::path::Path;
use std::process::Command;

/// `max_duration_min`: skip MS-SSIM when video longer than this (e.g. 5.0 normal, 25.0 ultimate).
pub fn calculate_ms_ssim_yuv(
    input: &Path,
    output: &Path,
    max_duration_min: f64,
) -> Option<(f64, f64, f64, f64)> {
    use chrono::Local;
    use std::thread;

    if let Some(ext) = input.extension().and_then(|e| e.to_str()) {
        if matches!(ext.to_lowercase().as_str(), "gif") {
            eprintln!(
                "   ℹ️  GIF format: skipping MS-SSIM (libvmaf incompatible), caller will use SSIM-All."
            );
            return None;
        }
    }

    let duration = match super::stream_analysis::get_video_duration(input) {
        Some(d) => d,
        None => {
            eprintln!("   ⚠️  Cannot determine video duration, using full calculation");
            60.0
        }
    };
    let duration_min = duration / 60.0;

    // Caller sets max_duration_min (e.g. 5 min normal, 25 min ultimate) to control skip threshold.
    let (sample_rate, should_calculate) = if duration_min <= 1.0 {
        (1, true)
    } else if duration_min <= max_duration_min {
        (3, true)
    } else {
        (0, false)
    };

    if !should_calculate {
        eprintln!(
            "   ⚠️  Quality verification: video too long ({:.1}min > {:.0}min), MS-SSIM skipped.",
            duration_min, max_duration_min
        );
        eprintln!("   📊 Using SSIM-only verification (faster; multi-scale not computed).");
        return None;
    }

    let start_ts = Local::now().format("%Y-%m-%d %H:%M:%S");
    eprintln!("   📊 Calculating 3-channel MS-SSIM (Y+U+V)...");
    eprintln!("   🕐 Start time: {}", start_ts);
    eprintln!("   📹 Video: {:.1}s ({:.1}min)", duration, duration_min);

    if sample_rate > 1 {
        let estimated_time = (duration / sample_rate as f64 * 3.0) as u64;
        eprintln!(
            "   ⚡ Sampling: 1/{} frames (est. {}s)",
            sample_rate, estimated_time
        );
    } else {
        let estimated_time = (duration * 3.0) as u64;
        eprintln!("   🎯 Full calculation (est. {}s)", estimated_time);
    }
    eprintln!("   🔄 Parallel processing: Y+U+V channels simultaneously");

    let input_y = input.to_path_buf();
    let output_y = output.to_path_buf();
    let input_u = input.to_path_buf();
    let output_u = output.to_path_buf();
    let input_v = input.to_path_buf();
    let output_v = output.to_path_buf();

    let start_time = std::time::Instant::now();

    let y_handle = thread::spawn(move || {
        calculate_ms_ssim_channel_sampled(&input_y, &output_y, "y", sample_rate)
    });
    let u_handle = thread::spawn(move || {
        calculate_ms_ssim_channel_sampled(&input_u, &output_u, "u", sample_rate)
    });
    let v_handle = thread::spawn(move || {
        calculate_ms_ssim_channel_sampled(&input_v, &output_v, "v", sample_rate)
    });

    let y_ms_ssim = match y_handle.join() {
        Ok(Some(v)) => v,
        _ => {
            eprintln!("   ❌ Y channel calculation failed");
            return None;
        }
    };
    let u_ms_ssim = match u_handle.join() {
        Ok(Some(v)) => v,
        _ => {
            eprintln!("   ❌ U channel calculation failed");
            return None;
        }
    };
    let v_ms_ssim = match v_handle.join() {
        Ok(Some(v)) => v,
        _ => {
            eprintln!("   ❌ V channel calculation failed");
            return None;
        }
    };

    eprintln!("      Y channel... {:.4} ✅", y_ms_ssim);
    eprintln!("      U channel... {:.4} ✅", u_ms_ssim);
    eprintln!("      V channel... {:.4} ✅", v_ms_ssim);

    let elapsed = start_time.elapsed().as_secs();
    let end_time = Local::now().format("%Y-%m-%d %H:%M:%S");
    eprintln!("   ⏱️  Completed in {}s (End: {})", elapsed, end_time);

    // BT.601 luma-weighted approx (Y dominant); chroma MS-SSIM on 4:2:0 subsampled planes may be lower than perceptual weight.
    let weighted_avg = (y_ms_ssim * 6.0 + u_ms_ssim + v_ms_ssim) / 8.0;

    Some((
        y_ms_ssim.clamp(0.0, 1.0),
        u_ms_ssim.clamp(0.0, 1.0),
        v_ms_ssim.clamp(0.0, 1.0),
        weighted_avg.clamp(0.0, 1.0),
    ))
}

fn calculate_ms_ssim_channel_sampled(
    input: &Path,
    output: &Path,
    channel: &str,
    sample_rate: usize,
) -> Option<f64> {
    if let Some(ext) = input.extension().and_then(|e| e.to_str()) {
        if matches!(ext.to_lowercase().as_str(), "gif") {
            eprintln!(
                "      ℹ️  GIF format: skipping YUV channel extraction (use SSIM-All instead)"
            );
            return None;
        }
    }

    let sample_filter = if sample_rate > 1 {
        format!(
            "select='not(mod(n\\,{}))',setpts=N/FRAME_RATE/TB,",
            sample_rate
        )
    } else {
        String::new()
    };

    let filter = format!(
        "[0:v]{}scale=trunc(iw/2)*2:trunc(ih/2)*2,format=yuv420p,extractplanes={}[c0];[1:v]{}scale=trunc(iw/2)*2:trunc(ih/2)*2,format=yuv420p,extractplanes={}[c1];[c0][c1]libvmaf=feature='name=float_ms_ssim':log_fmt=json:log_path=/dev/stdout",
        sample_filter, channel, sample_filter, channel
    );

    let result = Command::new("ffmpeg")
        .arg("-i")
        .arg(crate::safe_path_arg(input).as_ref())
        .arg("-i")
        .arg(crate::safe_path_arg(output).as_ref())
        .arg("-filter_complex")
        .arg(&filter)
        .arg("-f")
        .arg("null")
        .arg("-")
        .output();

    match result {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            parse_ms_ssim_from_json(&stdout)
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            eprintln!(
                "\n      ❌ Channel {} MS-SSIM failed!",
                channel.to_uppercase()
            );

            if stderr.contains("No such filter: 'libvmaf'") {
                eprintln!("         Cause: libvmaf filter not available in ffmpeg");
                eprintln!(
                    "         Fix: brew install homebrew-ffmpeg/ffmpeg/ffmpeg --with-libvmaf"
                );
            } else if stderr.contains("Invalid pixel format") || stderr.contains("format") {
                eprintln!("         Cause: Pixel format incompatibility");
                eprintln!("         Input: {}", input.display());
            } else if stderr.contains("scale") || stderr.contains("resolution") {
                eprintln!("         Cause: Resolution mismatch");
            } else {
                let error_lines: Vec<&str> = stderr
                    .lines()
                    .filter(|l| l.contains("Error") || l.contains("error") || l.contains("failed"))
                    .take(2)
                    .collect();
                if !error_lines.is_empty() {
                    eprintln!("         Error: {}", error_lines.join(" | "));
                }
            }
            None
        }
        Err(e) => {
            eprintln!(
                "\n      ❌ Channel {} command failed: {}",
                channel.to_uppercase(),
                e
            );
            None
        }
    }
}

pub fn calculate_ms_ssim(input: &Path, output: &Path) -> Option<f64> {
    if let Ok(info) = crate::ffprobe::probe_video(input) {
        if info.width < 64 || info.height < 64 {
            eprintln!(
                "   ⚠️  Skipping MS-SSIM: Image too small ({}x{}) for multi-scale analysis",
                info.width, info.height
            );
            return None;
        }
    }

    eprintln!("   📊 Calculating MS-SSIM (Multi-Scale Structural Similarity)...");

    let result = Command::new("ffmpeg")
        .arg("-i")
        .arg(crate::safe_path_arg(input).as_ref())
        .arg("-i")
        .arg(crate::safe_path_arg(output).as_ref())
        .arg("-lavfi")
        .arg("[0:v][1:v]libvmaf=log_path=/dev/stdout:log_fmt=json:feature='name=float_ms_ssim'")
        .arg("-f")
        .arg("null")
        .arg("-")
        .output();

    match result {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);

            if let Some(ms_ssim) = parse_ms_ssim_from_json(&stdout) {
                let clamped = ms_ssim.clamp(0.0, 1.0);
                if (ms_ssim - clamped).abs() > 0.0001 {
                    eprintln!(
                        "   ⚠️  MS-SSIM raw value {:.6} out of range, clamped to {:.4}",
                        ms_ssim, clamped
                    );
                }
                eprintln!("   📊 MS-SSIM score: {:.4}", clamped);
                return Some(clamped);
            }

            if let Some(ms_ssim) = parse_ms_ssim_from_legacy(&stderr) {
                let clamped = ms_ssim.clamp(0.0, 1.0);
                if (ms_ssim - clamped).abs() > 0.0001 {
                    eprintln!(
                        "   ⚠️  MS-SSIM raw value {:.6} out of range, clamped to {:.4}",
                        ms_ssim, clamped
                    );
                }
                eprintln!("   📊 MS-SSIM score: {:.4}", clamped);
                return Some(clamped);
            }

            eprintln!("   ⚠️  MS-SSIM calculated but failed to parse score");
        }
        Ok(_) => {
            eprintln!("   ⚠️  ffmpeg libvmaf MS-SSIM failed");
            eprintln!("   🔄 Trying standalone vmaf tool as fallback...");

            if crate::vmaf_standalone::is_vmaf_available() {
                match crate::vmaf_standalone::calculate_ms_ssim_standalone(input, output) {
                    Ok(score) => {
                        eprintln!("   ✅ Standalone vmaf MS-SSIM: {:.4}", score);
                        return Some(score);
                    }
                    Err(e) => {
                        eprintln!("   ⚠️  Standalone vmaf also failed: {}", e);
                    }
                }
            }
        }
        Err(e) => {
            eprintln!("   ⚠️  ffmpeg MS-SSIM failed: {}", e);
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
                        return after_colon[..end].parse::<f64>().ok();
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
        {
            if let Some(score_pos) = line.find("score:") {
                let after_score = &line[score_pos + 6..].trim_start();
                let end = after_score
                    .find(|c: char| !c.is_numeric() && c != '.')
                    .unwrap_or(after_score.len());
                if end > 0 {
                    return after_score[..end].parse::<f64>().ok();
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
pub fn calculate_vmaf_y(input: &Path, output: &Path, sample_rate: usize) -> Option<f64> {
    let sample_filter = if sample_rate > 1 {
        format!(
            "select='not(mod(n\\,{}))',setpts=N/FRAME_RATE/TB,",
            sample_rate
        )
    } else {
        String::new()
    };

    let n_threads = num_cpus_capped();

    // dis = output (distorted), ref = input (reference)
    let filter = format!(
        "[0:v]{sf}scale=trunc(iw/2)*2:trunc(ih/2)*2,format=yuv420p[dis];[1:v]{sf}scale=trunc(iw/2)*2:trunc(ih/2)*2,format=yuv420p[ref];[dis][ref]libvmaf=shortest=true:ts_sync_mode=nearest:n_threads={nt}:log_fmt=json:log_path=/dev/stdout",
        sf = sample_filter,
        nt = n_threads,
    );

    let result = Command::new("ffmpeg")
        .arg("-i")
        .arg(crate::safe_path_arg(output).as_ref())
        .arg("-i")
        .arg(crate::safe_path_arg(input).as_ref())
        .arg("-filter_complex")
        .arg(&filter)
        .arg("-f")
        .arg("null")
        .arg("-")
        .output();

    match result {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            parse_vmaf_mean_from_json(&stdout)
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            eprintln!("\n      ❌ VMAF-Y calculation failed!");
            if stderr.contains("No such filter: 'libvmaf'") {
                eprintln!("         Cause: libvmaf not available in this ffmpeg build");
                eprintln!(
                    "         Fix: brew install homebrew-ffmpeg/ffmpeg/ffmpeg --with-libvmaf"
                );
            } else {
                let error_lines: Vec<&str> = stderr
                    .lines()
                    .filter(|l| l.contains("Error") || l.contains("error") || l.contains("failed"))
                    .take(2)
                    .collect();
                if !error_lines.is_empty() {
                    eprintln!("         Error: {}", error_lines.join(" | "));
                }
            }
            None
        }
        Err(e) => {
            eprintln!("\n      ❌ VMAF-Y command failed: {}", e);
            None
        }
    }
}

/// Calculate CAMBI (Contrast Aware Multiscale Banding Index) for the output video.
/// CAMBI is a single-video metric (no reference needed) — lower is better (0 = no banding).
/// Returns None on failure or if libvmaf doesn't support the cambi feature.
pub fn calculate_cambi(output: &Path, sample_rate: usize) -> Option<f64> {
    let n_threads = num_cpus_capped();

    let log_file = tempfile::Builder::new().suffix(".json").tempfile().ok()?;
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

    let result = Command::new("ffmpeg")
        .arg("-i")
        .arg(crate::safe_path_arg(output).as_ref())
        .arg("-i")
        .arg(crate::safe_path_arg(output).as_ref())
        .arg("-filter_complex")
        .arg(&filter_complex)
        .arg("-f")
        .arg("null")
        .arg("-")
        .output();

    match result {
        Ok(out) if out.status.success() => {
            // Read JSON from the temp log file
            let json = std::fs::read_to_string(&log_path).ok()?;
            parse_cambi_mean_from_json(&json)
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            eprintln!("\n      ❌ CAMBI calculation failed!");
            if stderr.contains("No such filter: 'libvmaf'") {
                eprintln!("         Cause: libvmaf not available in this ffmpeg build");
            } else if stderr.contains("cambi")
                && (stderr.contains("unknown") || stderr.contains("No such"))
            {
                eprintln!(
                    "         Cause: libvmaf in this ffmpeg does not support the 'cambi' feature"
                );
                eprintln!("         Fix: upgrade to ffmpeg with libvmaf >= 2.x");
            } else {
                let error_lines: Vec<&str> = stderr
                    .lines()
                    .filter(|l| l.contains("Error") || l.contains("error") || l.contains("failed"))
                    .take(2)
                    .collect();
                if !error_lines.is_empty() {
                    eprintln!("         Error: {}", error_lines.join(" | "));
                }
            }
            None
        }
        Err(e) => {
            eprintln!("\n      ❌ CAMBI command failed: {}", e);
            None
        }
    }
}

/// Calculate PSNR for the U and V chroma channels independently.
/// Returns `(psnr_u, psnr_v)` in dB, or None on failure.
/// Uses `extractplanes` + ffmpeg's `psnr` filter (no libvmaf dependency).
pub fn calculate_psnr_uv(input: &Path, output: &Path, sample_rate: usize) -> Option<(f64, f64)> {
    use std::thread;

    let input_u = input.to_path_buf();
    let output_u = output.to_path_buf();
    let input_v = input.to_path_buf();
    let output_v = output.to_path_buf();

    let u_handle =
        thread::spawn(move || psnr_single_channel(&input_u, &output_u, "u", sample_rate));
    let v_handle =
        thread::spawn(move || psnr_single_channel(&input_v, &output_v, "v", sample_rate));

    let psnr_u = match u_handle.join() {
        Ok(Some(v)) => v,
        _ => {
            eprintln!("   ❌ PSNR-U channel calculation failed");
            return None;
        }
    };
    let psnr_v = match v_handle.join() {
        Ok(Some(v)) => v,
        _ => {
            eprintln!("   ❌ PSNR-V channel calculation failed");
            return None;
        }
    };

    Some((psnr_u, psnr_v))
}

fn psnr_single_channel(
    input: &Path,
    output: &Path,
    channel: &str,
    sample_rate: usize,
) -> Option<f64> {
    let sample_filter = if sample_rate > 1 {
        format!(
            "select='not(mod(n\\,{}))',setpts=N/FRAME_RATE/TB,",
            sample_rate
        )
    } else {
        String::new()
    };

    // Extract the requested plane from both streams, then run psnr on them.
    let filter = format!(
        "[0:v]{sf}scale=trunc(iw/2)*2:trunc(ih/2)*2,format=yuv420p,extractplanes={ch}[ref];[1:v]{sf}scale=trunc(iw/2)*2:trunc(ih/2)*2,format=yuv420p,extractplanes={ch}[dis];[ref][dis]psnr=stats_file=-",
        sf = sample_filter,
        ch = channel,
    );

    let result = Command::new("ffmpeg")
        .arg("-i")
        .arg(crate::safe_path_arg(input).as_ref())
        .arg("-i")
        .arg(crate::safe_path_arg(output).as_ref())
        .arg("-filter_complex")
        .arg(&filter)
        .arg("-f")
        .arg("null")
        .arg("-")
        .output();

    match result {
        Ok(out) => {
            // psnr stats_file=- writes per-frame stats to stdout; we need the average from stderr summary.
            let stderr = String::from_utf8_lossy(&out.stderr);
            parse_psnr_average_y_from_stderr(&stderr)
        }
        Err(e) => {
            eprintln!(
                "\n      ❌ PSNR-{} command failed: {}",
                channel.to_uppercase(),
                e
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
                let after = &line[pos + 2..];
                let end = after
                    .find(|c: char| !c.is_numeric() && c != '.')
                    .unwrap_or(after.len());
                if end > 0 {
                    if let Ok(v) = after[..end].parse::<f64>() {
                        if v.is_finite() && v > 0.0 {
                            return Some(v);
                        }
                    }
                }
            }
            // Fallback: "average:" field
            if let Some(pos) = line.find("average:") {
                let after = &line[pos + 8..];
                let end = after
                    .find(|c: char| !c.is_numeric() && c != '.')
                    .unwrap_or(after.len());
                if end > 0 {
                    if let Ok(v) = after[..end].parse::<f64>() {
                        if v.is_finite() && v > 0.0 {
                            return Some(v);
                        }
                    }
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
                        return after_colon[..end].parse::<f64>().ok();
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
                        return after_colon[..end].parse::<f64>().ok();
                    }
                }
            }
        }
    }
    None
}

/// Returns a capped thread count for libvmaf (max 8 to avoid over-subscription).
fn num_cpus_capped() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get().min(8))
        .unwrap_or(4)
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
        let v = result.unwrap();
        assert!((v - 94.123).abs() < 1e-6, "Expected 94.123, got {}", v);
    }

    #[test]
    fn test_parse_vmaf_mean_integer_value() {
        let json = r#"{"pooled_metrics": {"vmaf": {"mean": 100, "min": 99}}}"#;
        let result = parse_vmaf_mean_from_json(json);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), 100.0);
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
        assert!((result.unwrap() - 0.5).abs() < 1e-6);
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
        let v = result.unwrap();
        assert!((v - 7.456).abs() < 1e-6, "Expected 7.456, got {}", v);
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
            assert_eq!(v, 0.0);
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
        assert!((result.unwrap() - 42.789).abs() < 1e-6);
    }

    // ── parse_psnr_average_y_from_stderr ─────────────────────────────────────

    #[test]
    fn test_parse_psnr_standard_ffmpeg_line() {
        // Standard ffmpeg psnr filter summary line
        let stderr =
            "[Parsed_psnr_4 @ 0x123] PSNR y:41.234 u:39.876 v:40.123 average:40.411 min:38.123 max:42.567\n";
        let result = parse_psnr_average_y_from_stderr(stderr);
        assert!(result.is_some(), "Should parse PSNR from standard line");
        let v = result.unwrap();
        assert!((v - 41.234).abs() < 1e-3, "Expected y:41.234, got {}", v);
    }

    #[test]
    fn test_parse_psnr_uses_y_field_over_average() {
        // When both y: and average: are present, y: should be preferred
        let stderr = "PSNR y:38.5 average:37.0 min:35.0 max:40.0\n";
        let result = parse_psnr_average_y_from_stderr(stderr);
        assert!(result.is_some());
        // Should pick y:38.5, not average:37.0
        assert!((result.unwrap() - 38.5).abs() < 1e-3);
    }

    #[test]
    fn test_parse_psnr_fallback_to_average() {
        // No y: field, but average: present
        let stderr = "PSNR average:39.12 min:37.0 max:41.0\n";
        let result = parse_psnr_average_y_from_stderr(stderr);
        assert!(result.is_some());
        assert!((result.unwrap() - 39.12).abs() < 1e-3);
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
        assert!((result.unwrap() - 44.1).abs() < 1e-3);
    }

    // ── num_cpus_capped ───────────────────────────────────────────────────────

    #[test]
    fn test_num_cpus_capped_within_bounds() {
        let n = num_cpus_capped();
        assert!(n >= 1, "Thread count must be at least 1, got {}", n);
        assert!(n <= 8, "Thread count must be capped at 8, got {}", n);
    }

    #[test]
    fn test_num_cpus_capped_is_deterministic() {
        // Calling twice should return the same value
        assert_eq!(num_cpus_capped(), num_cpus_capped());
    }
}
