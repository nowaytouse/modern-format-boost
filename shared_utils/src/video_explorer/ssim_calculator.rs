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
            duration_min,
            max_duration_min
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

    let y_handle = thread::spawn(move || calculate_ms_ssim_channel_sampled(&input_y, &output_y, "y", sample_rate));
    let u_handle = thread::spawn(move || calculate_ms_ssim_channel_sampled(&input_u, &output_u, "u", sample_rate));
    let v_handle = thread::spawn(move || calculate_ms_ssim_channel_sampled(&input_v, &output_v, "v", sample_rate));

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
        "[0:v]{}format=yuv420p,extractplanes={}[c0];[1:v]{}format=yuv420p,extractplanes={}[c1];[c0][c1]libvmaf=feature='name=float_ms_ssim':log_fmt=json:log_path=/dev/stdout",
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
