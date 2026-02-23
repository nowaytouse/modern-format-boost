//! Dynamic GPU-to-CPU CRF mapping

use anyhow::{Context, Result};
use std::path::Path;

#[derive(Debug, Clone)]
pub struct AnchorPoint {
    pub crf: f32,
    pub gpu_size: u64,
    pub cpu_size: u64,
    pub size_ratio: f64,
}

#[derive(Debug, Clone)]
pub struct DynamicCrfMapper {
    pub anchors: Vec<AnchorPoint>,
    pub input_size: u64,
    pub calibrated: bool,
}

impl DynamicCrfMapper {
    pub fn new(input_size: u64) -> Self {
        Self {
            anchors: Vec::new(),
            input_size,
            calibrated: false,
        }
    }

    pub fn add_anchor(&mut self, crf: f32, gpu_size: u64, cpu_size: u64) {
        let size_ratio = cpu_size as f64 / gpu_size as f64;
        self.anchors.push(AnchorPoint {
            crf,
            gpu_size,
            cpu_size,
            size_ratio,
        });
        self.calibrated = !self.anchors.is_empty();
    }

    fn calculate_offset_from_ratio(size_ratio: f64) -> f32 {
        if size_ratio < 0.70 {
            4.0
        } else if size_ratio < 0.80 {
            3.5
        } else if size_ratio < 0.90 {
            3.0
        } else {
            2.5
        }
    }

    pub fn gpu_to_cpu(&self, gpu_crf: f32, base_offset: f32) -> (f32, f64) {
        if self.anchors.is_empty() {
            return (gpu_crf + base_offset, 0.5);
        }

        if self.anchors.len() == 1 {
            let offset = Self::calculate_offset_from_ratio(self.anchors[0].size_ratio);
            return (gpu_crf + offset, 0.75);
        }

        let p1 = &self.anchors[0];
        let p2 = &self.anchors[1];

        let offset1 = Self::calculate_offset_from_ratio(p1.size_ratio);
        let offset2 = Self::calculate_offset_from_ratio(p2.size_ratio);

        let t = if (p2.crf - p1.crf).abs() > 0.1 {
            ((gpu_crf - p1.crf) / (p2.crf - p1.crf)).clamp(0.0, 1.5)
        } else {
            0.5
        };

        let interpolated_offset = offset1 + t * (offset2 - offset1);
        let confidence = 0.85;

        (
            (gpu_crf + interpolated_offset).clamp(10.0, 51.0),
            confidence,
        )
    }

    pub fn print_calibration_report(&self) {
        if !crate::progress_mode::is_verbose_mode() {
            return;
        }
        if self.anchors.is_empty() {
            eprintln!("⚠️ Dynamic mapping: No calibration data, using static offset");
            return;
        }

        eprintln!("┌─────────────────────────────────────────────────────");
        eprintln!("│ Dynamic GPU→CPU Mapping Calibration (v5.61)");
        eprintln!("├─────────────────────────────────────────────────────");

        for (i, anchor) in self.anchors.iter().enumerate() {
            let offset = Self::calculate_offset_from_ratio(anchor.size_ratio);
            eprintln!("│ Anchor {}: CRF {:.1}", i + 1, anchor.crf);
            eprintln!("│   GPU: {} bytes", anchor.gpu_size);
            eprintln!("│   CPU: {} bytes", anchor.cpu_size);
            eprintln!(
                "│   Ratio: {:.3} → Offset: +{:.1}",
                anchor.size_ratio, offset
            );
        }

        eprintln!("└─────────────────────────────────────────────────────");
    }
}

pub fn quick_calibrate(
    input: &Path,
    input_size: u64,
    encoder: super::VideoEncoder,
    vf_args: &[String],
    gpu_encoder: &str,
    sample_duration: f32,
) -> Result<DynamicCrfMapper> {
    use std::fs;
    use std::process::Command;

    let mut mapper = DynamicCrfMapper::new(input_size);

    let is_gif_input = crate::ffprobe::probe_video(input)
        .map(|p| p.format_name.eq_ignore_ascii_case("gif"))
        .unwrap_or(false);
    if is_gif_input {
        crate::verbose_eprintln!(
            "   GIF detected: using FFmpeg libx265 path for calibration (no Y4M pipeline)"
        );
    }

    let calibration_crfs = vec![20.0_f32, 18.0, 22.0];
    let mut calibration_success = false;

    for (attempt, anchor_crf) in calibration_crfs.iter().enumerate() {
        crate::verbose_eprintln!(
            "Dynamic calibration attempt {}/{}: Testing CRF {:.1}...",
            attempt + 1,
            calibration_crfs.len(),
            anchor_crf
        );

        let temp_gpu_file = tempfile::Builder::new()
            .suffix(".mp4")
            .tempfile()
            .context("Failed to create temp file")?;
        let temp_cpu_file = tempfile::Builder::new()
            .suffix(".mp4")
            .tempfile()
            .context("Failed to create temp file")?;
        let temp_gpu = temp_gpu_file.path().to_path_buf();
        let temp_cpu = temp_cpu_file.path().to_path_buf();

        let gpu_result = Command::new("ffmpeg")
            .arg("-y")
            .arg("-t")
            .arg(format!("{}", sample_duration.min(10.0)))
            .arg("-i")
            .arg(crate::safe_path_arg(input).as_ref())
            .arg("-c:v")
            .arg(gpu_encoder)
            .arg("-crf")
            .arg(format!("{:.0}", anchor_crf))
            .arg("-c:a")
            .arg("copy")
            .arg(&temp_gpu)
            .output();

        let gpu_size = match gpu_result {
            Ok(out) if out.status.success() => {
                fs::metadata(&temp_gpu).map(|m| m.len()).unwrap_or(0)
            }
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                eprintln!("   ❌ GPU calibration failed for CRF {:.1}", anchor_crf);
                if stderr.contains("No such encoder") {
                    eprintln!("      Cause: GPU encoder '{}' not available", gpu_encoder);
                } else if stderr.contains("Invalid") {
                    eprintln!("      Cause: Invalid parameters");
                }
                continue;
            }
            Err(e) => {
                eprintln!("   ❌ GPU calibration command failed: {}", e);
                continue;
            }
        };

        if gpu_size == 0 {
            eprintln!("   ❌ GPU output file is empty");
            let _ = fs::remove_file(&temp_gpu);
            continue;
        }

        let max_threads = crate::thread_manager::get_ffmpeg_threads();

        let cpu_size = if encoder == super::VideoEncoder::Hevc && is_gif_input {
            let mut cpu_cmd = Command::new("ffmpeg");
            cpu_cmd
                .arg("-y")
                .arg("-t")
                .arg(format!("{}", sample_duration.min(10.0)))
                .arg("-i")
                .arg(crate::safe_path_arg(input).as_ref())
                .arg("-an")
                .arg("-vf")
                .arg("scale=trunc(iw/2)*2:trunc(ih/2)*2:flags=lanczos,format=yuv420p")
                .arg("-c:v")
                .arg("libx265")
                .arg("-crf")
                .arg(format!("{:.0}", anchor_crf));
            for arg in encoder.extra_args(max_threads) {
                cpu_cmd.arg(arg);
            }
            cpu_cmd.arg(&temp_cpu);
            match cpu_cmd.output() {
                Ok(out) if out.status.success() => {
                    fs::metadata(&temp_cpu).map(|m| m.len()).unwrap_or(0)
                }
                Ok(out) => {
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    eprintln!(
                        "   ❌ CPU calibration (GIF/libx265) failed for CRF {:.1}",
                        anchor_crf
                    );
                    let error_lines: Vec<&str> = stderr
                        .lines()
                        .filter(|l| {
                            l.contains("Error")
                                || l.contains("error")
                                || l.contains("Invalid")
                                || l.contains("failed")
                                || l.contains("No such")
                                || l.contains("cannot")
                        })
                        .take(2)
                        .collect();
                    if !error_lines.is_empty() {
                        eprintln!("      Cause: {}", error_lines.join(" | "));
                    }
                    let _ = fs::remove_file(&temp_gpu);
                    continue;
                }
                Err(e) => {
                    eprintln!("   ❌ CPU calibration (GIF) command failed: {}", e);
                    let _ = fs::remove_file(&temp_gpu);
                    continue;
                }
            }
        } else if encoder == super::VideoEncoder::Hevc {
            use crate::x265_encoder::{encode_with_x265, X265Config};

            let config = X265Config {
                crf: *anchor_crf,
                preset: "medium".to_string(),
                threads: max_threads,
                container: "mp4".to_string(),
                preserve_audio: true,
            };

            let temp_input_file = tempfile::Builder::new()
                .suffix(".y4m")
                .tempfile()
                .context("Failed to create temp file")?;
            let temp_input = temp_input_file.path().to_path_buf();
            let extract_result = Command::new("ffmpeg")
                .arg("-y")
                .arg("-t")
                .arg(format!("{}", sample_duration.min(10.0)))
                .arg("-i")
                .arg(crate::safe_path_arg(input).as_ref())
                .arg("-an")
                .arg("-vf")
                .arg("scale=trunc(iw/2)*2:trunc(ih/2)*2:flags=lanczos")
                .arg("-f")
                .arg("yuv4mpegpipe")
                .arg("-pix_fmt")
                .arg("yuv420p")
                .arg(crate::safe_path_arg(&temp_input).as_ref())
                .output();

            match extract_result {
                Ok(out) if out.status.success() => {}
                Ok(out) => {
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    eprintln!(
                        "   ❌ Failed to extract input sample for CRF {:.1}",
                        anchor_crf
                    );
                    let error_lines: Vec<&str> = stderr
                        .lines()
                        .filter(|l| {
                            l.contains("Error")
                                || l.contains("error")
                                || l.contains("Invalid")
                                || l.contains("failed")
                                || l.contains("No such")
                                || l.contains("cannot")
                        })
                        .take(2)
                        .collect();
                    if !error_lines.is_empty() {
                        eprintln!("      Cause: {}", error_lines.join(" | "));
                    }
                    let _ = fs::remove_file(&temp_gpu);
                    continue;
                }
                Err(e) => {
                    eprintln!("   ❌ Extract command failed: {}", e);
                    let _ = fs::remove_file(&temp_gpu);
                    continue;
                }
            }

            match encode_with_x265(&temp_input, &temp_cpu, &config, vf_args) {
                Ok(_) => {
                    let _ = fs::remove_file(&temp_input);
                    fs::metadata(&temp_cpu).map(|m| m.len()).unwrap_or(0)
                }
                Err(e) => {
                    eprintln!(
                        "   ❌ CPU x265 encoding failed for CRF {:.1}: {}",
                        anchor_crf, e
                    );
                    let _ = fs::remove_file(&temp_input);
                    let _ = fs::remove_file(&temp_gpu);
                    continue;
                }
            }
        } else {
            let mut cpu_cmd = Command::new("ffmpeg");
            cpu_cmd
                .arg("-y")
                .arg("-t")
                .arg(format!("{}", sample_duration.min(10.0)))
                .arg("-i")
                .arg(crate::safe_path_arg(input).as_ref())
                .arg("-c:v")
                .arg(encoder.ffmpeg_name())
                .arg("-crf")
                .arg(format!("{:.0}", anchor_crf));

            for arg in encoder.extra_args(max_threads) {
                cpu_cmd.arg(arg);
            }

            for arg in vf_args {
                if !arg.is_empty() {
                    cpu_cmd.arg("-vf").arg(arg);
                }
            }

            cpu_cmd.arg("-c:a").arg("copy").arg(&temp_cpu);

            let cpu_result = cpu_cmd.output();

            match cpu_result {
                Ok(out) if out.status.success() => {
                    fs::metadata(&temp_cpu).map(|m| m.len()).unwrap_or(0)
                }
                Ok(out) => {
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    eprintln!("   ❌ CPU encoding failed for CRF {:.1}", anchor_crf);
                    if stderr.contains("No such encoder") {
                        eprintln!("      Cause: CPU encoder not available");
                    }
                    let _ = fs::remove_file(&temp_gpu);
                    continue;
                }
                Err(e) => {
                    eprintln!("   ❌ CPU command failed: {}", e);
                    let _ = fs::remove_file(&temp_gpu);
                    continue;
                }
            }
        };

        let _ = fs::remove_file(&temp_gpu);
        let _ = fs::remove_file(&temp_cpu);

        if gpu_size > 0 && cpu_size > 0 {
            mapper.add_anchor(*anchor_crf, gpu_size, cpu_size);

            let ratio = cpu_size as f64 / gpu_size as f64;
            let _offset = DynamicCrfMapper::calculate_offset_from_ratio(ratio);

            crate::verbose_eprintln!("   ✅ Calibration successful at CRF {:.1}", anchor_crf);
            crate::verbose_eprintln!(
                "      GPU: {} bytes, CPU: {} bytes (ratio: {:.2})",
                gpu_size, cpu_size, ratio
            );
            calibration_success = true;
            break;
        }
    }

    if !calibration_success {
        eprintln!("⚠️ All CPU calibration attempts failed, using static offset");
        eprintln!("   Tried CRF values: {:?}", calibration_crfs);
        eprintln!("   This may affect GPU→CPU mapping accuracy");
        return Ok(mapper);
    }

    {
        let ratio = mapper.anchors[0].cpu_size as f64 / mapper.anchors[0].gpu_size as f64;
        let offset = DynamicCrfMapper::calculate_offset_from_ratio(ratio);
        let gpu_size = mapper.anchors[0].gpu_size;
        let cpu_size = mapper.anchors[0].cpu_size;
        crate::verbose_eprintln!(
            "✅ Calibration complete: GPU {} → CPU {} (ratio {:.3}, offset +{:.1})",
            gpu_size, cpu_size, ratio, offset
        );
    }

    Ok(mapper)
}
