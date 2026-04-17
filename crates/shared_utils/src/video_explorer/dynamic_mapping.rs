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
    #[must_use]
    pub const fn new(input_size: u64) -> Self {
        Self {
            anchors: Vec::new(),
            input_size,
            calibrated: false,
        }
    }

    pub fn add_anchor(&mut self, crf: f32, gpu_size: u64, cpu_size: u64) {
        if gpu_size == 0 {
            return;
        }
        let size_ratio = cpu_size as f64 / gpu_size as f64;
        self.anchors.push(AnchorPoint {
            crf,
            gpu_size,
            cpu_size,
            size_ratio,
        });
        self.calibrated = true;
    }

    fn calculate_offset_from_ratio(size_ratio: f64) -> f32 {
        if size_ratio >= 1.0 {
            // CPU output larger than GPU at same CRF; don't add positive offset.
            0.0
        } else if size_ratio < 0.70 {
            4.0
        } else if size_ratio < 0.80 {
            3.5
        } else if size_ratio < 0.90 {
            3.0
        } else {
            2.5
        }
    }

    /// Maps GPU CRF to CPU CRF. `max_crf`: HEVC/H264 use 51.0, AV1 use 63.0.
    #[must_use]
    pub fn gpu_to_cpu(&self, gpu_crf: f32, base_offset: f32, max_crf: f32) -> (f32, f64) {
        if self.anchors.is_empty() {
            return ((gpu_crf + base_offset).clamp(10.0, max_crf), 0.5);
        }

        if self.anchors.len() == 1 {
            let offset = Self::calculate_offset_from_ratio(self.anchors[0].size_ratio);
            return ((gpu_crf + offset).clamp(10.0, max_crf), 0.75);
        }

        // Multi-anchor interpolation (currently unused: quick_calibrate stops after first success).
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
            (gpu_crf + interpolated_offset).clamp(10.0, max_crf),
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

/// Quickly calibrate a CRF value using a GPU-accelerated coarse search.
///
/// # Errors
/// Returns an error if the search fails.
///
/// # Panics
/// Panics if the input file path is not a valid UTF-8 string.
pub fn quick_calibrate(
    input: &Path,
    input_size: u64,
    encoder: super::VideoEncoder,
    vf_args: &[String],
    gpu_encoder: &str,
    sample_duration: f32,
    ultimate_mode: bool,
    apple_compat: bool,
) -> Result<DynamicCrfMapper> {
    use std::fs;

    let mut mapper = DynamicCrfMapper::new(input_size);

    let is_gif_input =
        crate::ffprobe::probe_video(input).is_ok_and(|p| p.format_name.eq_ignore_ascii_case("gif"));
    if is_gif_input {
        crate::verbose_eprintln!(
            "   GIF detected: using FFmpeg libx265 path for calibration (no Y4M pipeline)"
        );
    }

    let calibration_crfs = vec![20.0_f32, 18.0, 22.0];
    let mut calibration_success = false;

    let cpu_sample_cap = if ultimate_mode { 30.0 } else { 15.0 };

    for (attempt, anchor_crf) in calibration_crfs.iter().enumerate() {
        crate::verbose_eprintln!(
            "Dynamic calibration attempt {}/{}: Testing CRF {:.1}...",
            attempt + 1,
            calibration_crfs.len(),
            anchor_crf
        );

        let gpu_test_file = tempfile::Builder::new()
            .suffix(".mp4")
            .tempfile()
            .context("Failed to create temp file")?;
        let cpu_test_file = tempfile::Builder::new()
            .suffix(".mp4")
            .tempfile()
            .context("Failed to create temp file")?;
        let gpu_path = gpu_test_file.path().to_path_buf();
        let cpu_path = cpu_test_file.path().to_path_buf();

        let mut gpu_builder = crate::ffmpeg_builder::FfmpegBuilder::new();
        gpu_builder.overwrite().input(input).codec_video(gpu_encoder);
        if apple_compat && encoder == super::VideoEncoder::Hevc {
            gpu_builder
                .arg(crate::constants::FFMPEG_ARG_TAG_VIDEO)
                .arg(crate::constants::FFMPEG_TAG_HVC1);
        }
        for arg in vf_args {
            gpu_builder.arg(arg);
        }
        gpu_builder
            .arg("-q:v")
            .arg(format!("{anchor_crf:.1}"))
            .arg("-t")
            .arg(sample_duration.to_string())
            .output(&gpu_path);

        let gpu_result = gpu_builder.build().output();

        let gpu_size = match gpu_result {
            Ok(out) if out.status.success() => fs::metadata(&gpu_path).map_or(0, |m| m.len()),
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                eprintln!("   ❌ GPU calibration failed for CRF {anchor_crf:.1}");
                if stderr.contains("No such encoder") {
                    eprintln!("      Cause: GPU encoder '{gpu_encoder}' not available");
                } else if stderr.contains("Invalid") {
                    eprintln!("      Cause: Invalid parameters");
                }
                continue;
            }
            Err(e) => {
                eprintln!("   ❌ GPU calibration command failed: {e}");
                continue;
            }
        };

        if gpu_size == 0 {
            eprintln!("   ❌ GPU output file is empty");
            continue;
        }

        let max_threads = crate::thread_manager::get_ffmpeg_threads();

        let cpu_size = if encoder == super::VideoEncoder::Hevc && is_gif_input {
            let mut cpu_builder = crate::ffmpeg_builder::FfmpegBuilder::new();
            cpu_builder
                .overwrite()
                .arg("-t")
                .arg(format!("{}", sample_duration.min(cpu_sample_cap)))
                .input(input)
                .codec_audio("none")
                .arg("-vf")
                .arg("scale=trunc(iw/2)*2:trunc(ih/2)*2:flags=lanczos,format=yuv420p")
                .codec_video("libx265")
                .arg("-crf")
                .arg(format!("{anchor_crf:.0}"));

            for arg in encoder.extra_args(max_threads, apple_compat) {
                cpu_builder.arg(arg);
            }

            let mut cpu_cmd = cpu_builder.output(&cpu_path).build();
            match cpu_cmd.output() {
                Ok(out) if out.status.success() => fs::metadata(&cpu_path).map_or(0, |m| m.len()),
                Ok(out) => {
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    eprintln!("   ❌ CPU calibration (GIF/libx265) failed for CRF {anchor_crf:.1}");
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
                    continue;
                }
                Err(e) => {
                    eprintln!("   ❌ CPU calibration (GIF) command failed: {e}");
                    continue;
                }
            }
        } else if encoder == super::VideoEncoder::Hevc {
            use crate::x265_encoder::{encode_with_x265, X265Config};

            // Probe the input to decide HDR-aware pix_fmt so the CPU calibration
            // encode doesn't silently downshift a 10-bit HDR source to 8-bit SDR.
            let probe = crate::ffprobe::probe_video(input).ok();
            let is_ten_bit = probe
                .as_ref()
                .is_some_and(|p| p.bit_depth >= 10);
            let pix_fmt = if is_ten_bit { "yuv420p10le" } else { "yuv420p" };

            let config = X265Config {
                crf: *anchor_crf,
                preset: crate::types::EncoderPreset::Medium.hevc_name().to_string(),
                threads: max_threads,
                container: "mp4".to_string(),
                preserve_audio: true,
                pix_fmt: pix_fmt.to_string(),
                ..Default::default()
            };

            let temp_input_file = tempfile::Builder::new()
                .suffix(".y4m")
                .tempfile()
                .context("Failed to create temp file")?;
            let temp_input = temp_input_file.path().to_path_buf();
            let temp_y4m = temp_input.to_str().unwrap();
            let extract_result = crate::ffmpeg_builder::FfmpegBuilder::new()
                .overwrite()
                .input(input)
                .arg("-vf")
                .arg("select=eq(n\\,0)")
                .arg("-pix_fmt")
                .arg(pix_fmt)
                .arg("-frames:v")
                .arg("1")
                .output(temp_y4m)
                .build()
                .output();

            match extract_result {
                Ok(out) if out.status.success() => {}
                Ok(out) => {
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    eprintln!("   ❌ Failed to extract input sample for CRF {anchor_crf:.1}");
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
                    continue;
                }
                Err(e) => {
                    eprintln!("   ❌ Extract command failed: {e}");
                    continue;
                }
            }

            // Guard: verify the y4m file was actually written with content.
            // ffmpeg can exit 0 on certain inputs (e.g. zero-duration sample, codec
            // mismatch) while producing an empty file.  x265 then reports
            // "unable to open input file" which looks like a file-not-found error
            // but is really a race/empty-file issue.  Skip this CRF attempt and
            // try the next one rather than propagating a misleading x265 error.
            let y4m_size = fs::metadata(&temp_input).map_or(0, |m| m.len());
            if y4m_size == 0 {
                eprintln!(
                    "   ❌ Extracted y4m sample is empty for CRF {anchor_crf:.1} (ffmpeg exited 0 but wrote nothing); skipping"
                );
                continue;
            }

            match encode_with_x265(&temp_input, &cpu_path, &config, vf_args) {
                Ok(_) => fs::metadata(&cpu_path).map_or(0, |m| m.len()),
                Err(e) => {
                    eprintln!("   ❌ CPU x265 encoding failed for CRF {anchor_crf:.1}: {e}");
                    continue;
                }
            }
        } else {
            // Non-HEVC CPU branch (AV1, H.264). Previously this path force-tonemapped
            // HDR→BT.709 via a `-vf zscale=p=bt709:t=bt709:m=bt709` on input that was
            // then silently shadowed by `-vf vf_joined` below (FFmpeg keeps only the
            // last `-vf`), and also routed to `-f null -`. Both bugs made the
            // cpu_size measurement either 0 bytes (null muxer) or an apples-to-oranges
            // comparison with the HDR GPU output, biasing the calibration ratio.
            //
            // Build a single proper CPU encode with the same filter chain the GPU
            // side used, writing to cpu_path so fs::metadata() can read its size.
            let mut cpu_builder = crate::ffmpeg_builder::FfmpegBuilder::new();
            cpu_builder
                .overwrite()
                .input(input)
                .arg("-t")
                .arg(sample_duration.to_string());

            let vf_joined: String = vf_args
                .iter()
                .filter(|s| !s.is_empty() && s.as_str() != "-vf")
                .cloned()
                .collect::<Vec<_>>()
                .join(",");
            if !vf_joined.is_empty() {
                cpu_builder.arg("-vf").arg(vf_joined);
            }

            cpu_builder
                .codec_video(encoder.ffmpeg_name())
                .arg("-crf")
                .arg(format!("{anchor_crf:.1}"));

            for arg in encoder.extra_args(max_threads, apple_compat) {
                cpu_builder.arg(arg);
            }

            cpu_builder.codec_audio("none");
            cpu_builder.output(&cpu_path);

            let cpu_result = cpu_builder.build().output();

            match cpu_result {
                Ok(out) if out.status.success() => fs::metadata(&cpu_path).map_or(0, |m| m.len()),
                Ok(out) => {
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    eprintln!("   ❌ CPU encoding failed for CRF {anchor_crf:.1}");
                    if stderr.contains("No such encoder") {
                        eprintln!("      Cause: CPU encoder not available");
                    }
                    continue;
                }
                Err(e) => {
                    eprintln!("   ❌ CPU command failed: {e}");
                    continue;
                }
            }
        };

        if gpu_size > 0 && cpu_size > 0 {
            mapper.add_anchor(*anchor_crf, gpu_size, cpu_size);

            let ratio = cpu_size as f64 / gpu_size as f64;

            crate::verbose_eprintln!("   ✅ Calibration successful at CRF {:.1}", anchor_crf);
            crate::verbose_eprintln!(
                "      GPU: {} bytes, CPU: {} bytes (ratio: {:.2})",
                gpu_size,
                cpu_size,
                ratio
            );
            calibration_success = true;
            break;
        }
    }

    if !calibration_success {
        eprintln!("⚠️ All CPU calibration attempts failed, using static offset");
        eprintln!("   Tried CRF values: {calibration_crfs:?}");
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
            gpu_size,
            cpu_size,
            ratio,
            offset
        );
    }

    Ok(mapper)
}
