//! Dynamic GPU-to-CPU CRF mapping

use crate::builder_base::ToolBuilder;
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
        let size_ratio =
            crate::numeric_cast::u64_to_f64(cpu_size) / crate::numeric_cast::u64_to_f64(gpu_size);
        self.anchors.push(AnchorPoint {
            crf,
            gpu_size,
            cpu_size,
            size_ratio,
        });
        self.calibrated = true;
    }

    fn calculate_offset_from_ratio(size_ratio: f64) -> f32 {
        use crate::constants::{
            DYNAMIC_MAPPING_OFFSET_DEFAULT, DYNAMIC_MAPPING_OFFSET_TIER_1,
            DYNAMIC_MAPPING_OFFSET_TIER_2, DYNAMIC_MAPPING_OFFSET_TIER_3,
            DYNAMIC_MAPPING_RATIO_TIER_1, DYNAMIC_MAPPING_RATIO_TIER_2,
            DYNAMIC_MAPPING_RATIO_TIER_3,
        };
        if size_ratio >= 1.0 {
            // CPU output larger than GPU at same CRF; don't add positive offset.
            0.0
        } else if size_ratio < DYNAMIC_MAPPING_RATIO_TIER_1 {
            DYNAMIC_MAPPING_OFFSET_TIER_1
        } else if size_ratio < DYNAMIC_MAPPING_RATIO_TIER_2 {
            DYNAMIC_MAPPING_OFFSET_TIER_2
        } else if size_ratio < DYNAMIC_MAPPING_RATIO_TIER_3 {
            DYNAMIC_MAPPING_OFFSET_TIER_3
        } else {
            DYNAMIC_MAPPING_OFFSET_DEFAULT
        }
    }

    /// Maps GPU CRF to CPU CRF. `max_crf`: HEVC/H264 use 51.0, AV1 use 63.0.
    #[must_use]
    pub fn gpu_to_cpu(&self, gpu_crf: f32, base_offset: f32, max_crf: f32) -> (f32, f64) {
        use crate::constants::{
            DYNAMIC_MAPPING_CONFIDENCE_LOW, DYNAMIC_MAPPING_CONFIDENCE_MEDIUM,
            DYNAMIC_MAPPING_MIN_CPU_CRF,
        };
        if self.anchors.is_empty() {
            return (
                (gpu_crf + base_offset).clamp(DYNAMIC_MAPPING_MIN_CPU_CRF, max_crf),
                DYNAMIC_MAPPING_CONFIDENCE_LOW,
            );
        }

        if self.anchors.len() == 1 {
            let offset = crate::media_conversion_gate::explore_dynamic_mapping_offset_or_zero(
                self.anchors.first().map(|a| a.size_ratio),
                "dynamic_mapping single anchor",
            );
            if !offset.is_finite() {
                return (
                    (gpu_crf + base_offset).clamp(DYNAMIC_MAPPING_MIN_CPU_CRF, max_crf),
                    DYNAMIC_MAPPING_CONFIDENCE_LOW,
                );
            }
            return (
                (gpu_crf + offset).clamp(DYNAMIC_MAPPING_MIN_CPU_CRF, max_crf),
                DYNAMIC_MAPPING_CONFIDENCE_MEDIUM,
            );
        }

        // Multi-anchor interpolation (currently unused: quick_calibrate stops after
        // first success).
        let [p1, p2, ..] = &self.anchors[..] else {
            return ((gpu_crf + base_offset).clamp(10.0, max_crf), 0.5);
        };

        let offset1 = Self::calculate_offset_from_ratio(p1.size_ratio);
        let offset2 = Self::calculate_offset_from_ratio(p2.size_ratio);

        let t = if (p2.crf - p1.crf).abs() > 0.1 {
            ((gpu_crf - p1.crf) / (p2.crf - p1.crf)).clamp(0.0, 1.5)
        } else {
            0.5
        };

        let interpolated_offset = f32::mul_add(t, offset2 - offset1, offset1);
        let confidence = crate::constants::DYNAMIC_MAPPING_CONFIDENCE_HIGH;

        (
            (gpu_crf + interpolated_offset)
                .clamp(crate::constants::DYNAMIC_MAPPING_MIN_CPU_CRF, max_crf),
            confidence,
        )
    }

    pub fn print_calibration_report(&self) {
        if !crate::progress_mode::is_verbose_mode() {
            return;
        }
        if self.anchors.is_empty() {
            crate::media_conversion_gate::explore_calibration_degraded_audit(
                "Dynamic mapping: No calibration data, using static offset",
            );
            return;
        }

        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_DYNAMIC,
            "┌─────────────────────────────────────────────────────"
        );
        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_DYNAMIC,
            "│ Dynamic GPU→CPU Mapping Calibration (v5.61)"
        );
        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_DYNAMIC,
            "├─────────────────────────────────────────────────────"
        );

        for (i, anchor) in self.anchors.iter().enumerate() {
            let offset = Self::calculate_offset_from_ratio(anchor.size_ratio);
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_DYNAMIC,
                format!("│ Anchor {}: CRF {:.1}", i + 1, anchor.crf)
            );
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_DYNAMIC,
                format!("│   GPU: {} bytes", anchor.gpu_size)
            );
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_DYNAMIC,
                format!("│   CPU: {} bytes", anchor.cpu_size)
            );
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_DYNAMIC,
                format!(
                    "│   Ratio: {:.3} → Offset: +{:.1}",
                    anchor.size_ratio, offset
                )
            );
        }

        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_DYNAMIC,
            "└─────────────────────────────────────────────────────"
        );
    }
}

fn collect_vf_filters(vf_args: &[String]) -> Vec<String> {
    let mut filters = Vec::new();
    let mut idx = 0;

    while idx < vf_args.len() {
        if let Some(arg) = vf_args.get(idx)
            && arg == "-vf"
            && let Some(val) = vf_args.get(idx + 1)
            && !val.is_empty()
        {
            filters.push(val.clone());
            idx += 2;
            continue;
        }
        idx += 1;
    }

    filters
}

fn build_calibration_filter_chain(
    vf_args: &[String],
    input_duration: Option<f64>,
    ultimate_mode: bool,
    tail_filters: &[String],
) -> String {
    let mut filters = Vec::new();

    if let Some(duration) = input_duration
        && let Some(sampling_filter) =
            crate::gpu_accel::build_multi_segment_sampling_filter(duration, ultimate_mode)
    {
        filters.push(sampling_filter);
    }

    filters.extend(collect_vf_filters(vf_args));
    filters.extend(tail_filters.iter().cloned());

    filters.join(",")
}

struct CalibrationContext<'a> {
    input: &'a Path,
    encoder: super::VideoEncoder,
    vf_args: &'a [String],
    gpu_encoder: &'a crate::gpu_accel::GpuEncoder,
    sample_duration: f32,
    ultimate_mode: bool,
    apple_compat: bool,
    probe: Option<crate::ffprobe::FFprobeResult>,
    input_kind: CalibrationInputKind,
    input_duration: f64,
    sampling_mode: CalibrationSamplingMode,
    base_calibration_filter: String,
    max_threads: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CalibrationInputKind {
    Gif,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CalibrationSamplingMode {
    MultiSegment,
    Timed,
}

impl<'a> CalibrationContext<'a> {
    fn new(
        input: &'a Path,
        encoder: super::VideoEncoder,
        vf_args: &'a [String],
        gpu_encoder: &'a crate::gpu_accel::GpuEncoder,
        sample_duration: f32,
        ultimate_mode: bool,
        apple_compat: bool,
    ) -> anyhow::Result<Self> {
        let probe = crate::ffprobe::probe_video(input).map_err(|err| {
            crate::media_conversion_gate::explore_calibration_degraded_audit(format!(
                "Failed to probe video during dynamic calibration (input={}): {err}",
                input.display()
            ));
            anyhow::anyhow!("dynamic calibration requires ffprobe: {err}")
        })?;
        let input_kind = if probe.format_name.eq_ignore_ascii_case("gif") {
            CalibrationInputKind::Gif
        } else {
            CalibrationInputKind::Other
        };
        let input_duration =
            crate::media_conversion_gate::explore_calibration_duration_optional(probe.duration)
                .ok_or_else(|| {
                    anyhow::anyhow!("dynamic calibration requires measured ffprobe duration")
                })?;
        let sampling_mode =
            if crate::gpu_accel::build_multi_segment_sampling_filter(input_duration, ultimate_mode)
                .is_some()
            {
                CalibrationSamplingMode::MultiSegment
            } else {
                CalibrationSamplingMode::Timed
            };
        let base_calibration_filter =
            build_calibration_filter_chain(vf_args, Some(input_duration), ultimate_mode, &[]);

        if input_kind == CalibrationInputKind::Gif {
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_DYNAMIC,
                "GIF detected: using FFmpeg libx265 path for calibration (no Y4M pipeline)"
            );
        }

        Ok(Self {
            input,
            encoder,
            vf_args,
            gpu_encoder,
            sample_duration,
            ultimate_mode,
            apple_compat,
            probe: Some(probe),
            input_kind,
            input_duration,
            sampling_mode,
            base_calibration_filter,
            max_threads: crate::thread_manager::get_ffmpeg_threads(),
        })
    }

    fn try_anchor(
        &self,
        anchor_crf: f32,
        gpu_path: &Path,
        cpu_path: &Path,
    ) -> Result<Option<(u64, u64)>> {
        let Some(gpu_size) = self.run_gpu_probe(anchor_crf, gpu_path)? else {
            return Ok(None);
        };
        if gpu_size == 0 {
            crate::log_failure!(
                crate::infra::static_logs::messages::LABEL_DYNAMIC,
                "GPU output file is empty"
            );
            return Ok(None);
        }

        let Some(cpu_size) = self.run_cpu_probe(anchor_crf, cpu_path) else {
            return Ok(None);
        };
        if cpu_size == 0 {
            return Ok(None);
        }

        Ok(Some((gpu_size, cpu_size)))
    }

    fn run_gpu_probe(&self, anchor_crf: f32, gpu_path: &Path) -> Result<Option<u64>> {
        let mut gpu_builder = crate::ffmpeg_builder::FfmpegBuilder::new();
        gpu_builder
            .overwrite()
            .input(self.input)
            .arg("-map")
            .arg("0:v:0")
            .arg("-an")
            .codec_video(self.gpu_encoder.ffmpeg_name());
        if self.apple_compat && self.encoder == super::VideoEncoder::Hevc {
            gpu_builder
                .arg(crate::constants::FFMPEG_ARG_TAG_VIDEO)
                .arg(crate::constants::FFMPEG_TAG_HVC1);
        }
        for arg in self.gpu_encoder.get_crf_args(anchor_crf)? {
            gpu_builder.arg(arg);
        }
        for arg in self.gpu_encoder.extra_args() {
            gpu_builder.arg(arg);
        }
        if !self.base_calibration_filter.is_empty() {
            gpu_builder.arg("-vf").arg(&self.base_calibration_filter);
        }
        if self.sampling_mode == CalibrationSamplingMode::Timed {
            gpu_builder.arg("-t").arg(self.sample_duration.to_string());
        }
        gpu_builder.output(gpu_path);

        match gpu_builder.build().output() {
            Ok(out) if out.status.success() => Ok(Self::read_probe_output_size(
                gpu_path,
                "GPU test file metadata",
            )),
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                crate::log_failure!(
                    crate::infra::static_logs::messages::LABEL_DYNAMIC,
                    format!("GPU calibration failed for CRF {anchor_crf:.1}")
                );
                if stderr.contains("No such encoder") {
                    crate::log_failure!(
                        crate::infra::static_logs::messages::LABEL_DYNAMIC,
                        format!(
                            "Cause: GPU encoder '{}' not available",
                            self.gpu_encoder.ffmpeg_name()
                        )
                    );
                } else if stderr.contains("Invalid") {
                    crate::log_failure!(
                        crate::infra::static_logs::messages::LABEL_DYNAMIC,
                        "Cause: Invalid parameters"
                    );
                }
                Ok(None)
            }
            Err(err) => {
                crate::log_failure!(
                    crate::infra::static_logs::messages::LABEL_DYNAMIC,
                    format!("GPU calibration command failed: {err}")
                );
                Ok(None)
            }
        }
    }

    fn run_cpu_probe(&self, anchor_crf: f32, cpu_path: &Path) -> Option<u64> {
        if self.encoder == super::VideoEncoder::Hevc && self.input_kind == CalibrationInputKind::Gif
        {
            return self.run_cpu_gif_hevc_probe(anchor_crf, cpu_path);
        }
        if self.encoder == super::VideoEncoder::Hevc {
            return self.run_cpu_x265_probe(anchor_crf, cpu_path);
        }
        self.run_cpu_generic_probe(anchor_crf, cpu_path)
    }

    fn run_cpu_gif_hevc_probe(&self, anchor_crf: f32, cpu_path: &Path) -> Option<u64> {
        let mut cpu_builder = crate::ffmpeg_builder::FfmpegBuilder::new();
        cpu_builder
            .overwrite()
            .input(self.input)
            .arg("-map")
            .arg("0:v:0")
            .codec_audio("none")
            .codec_video("libx265")
            .arg("-crf")
            .arg(format!("{anchor_crf:.0}"));
        let gif_filter = build_calibration_filter_chain(
            self.vf_args,
            Some(self.input_duration),
            self.ultimate_mode,
            &["scale=trunc(iw/2)*2:trunc(ih/2)*2:flags=lanczos,format=yuv420p".to_string()],
        );
        if !gif_filter.is_empty() {
            cpu_builder.arg("-vf").arg(gif_filter);
        }
        if self.sampling_mode == CalibrationSamplingMode::Timed {
            cpu_builder.arg("-t").arg(self.sample_duration.to_string());
        }

        for arg in self.encoder.extra_args(self.max_threads, self.apple_compat) {
            cpu_builder.arg(arg);
        }

        let mut cpu_cmd = cpu_builder.output(cpu_path).build();
        match cpu_cmd.output() {
            Ok(out) if out.status.success() => {
                Self::read_probe_output_size(cpu_path, "CPU test file metadata (GIF/x265)")
            }
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                crate::log_failure!(
                    crate::infra::static_logs::messages::LABEL_DYNAMIC,
                    format!("CPU calibration (GIF/libx265) failed for CRF {anchor_crf:.1}")
                );
                let error_lines: Vec<&str> = stderr
                    .lines()
                    .filter(|line| {
                        line.contains("Error")
                            || line.contains("error")
                            || line.contains("Invalid")
                            || line.contains("failed")
                            || line.contains("No such")
                            || line.contains("cannot")
                    })
                    .take(2)
                    .collect();
                if !error_lines.is_empty() {
                    crate::log_failure!(
                        crate::infra::static_logs::messages::LABEL_DYNAMIC,
                        format!("Cause: {}", error_lines.join(" | "))
                    );
                }
                None
            }
            Err(err) => {
                crate::log_failure!(
                    crate::infra::static_logs::messages::LABEL_DYNAMIC,
                    format!("CPU calibration (GIF) command failed: {err}")
                );
                None
            }
        }
    }

    fn run_cpu_x265_probe(&self, anchor_crf: f32, cpu_path: &Path) -> Option<u64> {
        use crate::x265_encoder::{X265Config, encode_with_x265};

        let pix_fmt = crate::media_conversion_gate::explore_calibration_pix_fmt_optional(
            self.probe.as_ref(),
        )?;
        let cpu_vf_args = vec![
            "-vf".to_string(),
            build_calibration_filter_chain(
                self.vf_args,
                Some(self.input_duration),
                self.ultimate_mode,
                &[
                    "pad=ceil(iw/2)*2:ceil(ih/2)*2:0:0".to_string(),
                    format!("format={pix_fmt}"),
                ],
            ),
        ];

        let config = X265Config {
            crf: anchor_crf,
            preset: crate::types::EncoderPreset::Medium.hevc_name().to_string(),
            threads: self.max_threads,
            container: "mp4".to_string(),
            sample_duration: (self.sampling_mode == CalibrationSamplingMode::Timed)
                .then_some(self.sample_duration),
            preserve_audio: false,
            pix_fmt: pix_fmt.to_string(),
            color_primaries: self.probe.as_ref().and_then(|p| p.color_primaries.clone()),
            color_trc: self.probe.as_ref().and_then(|p| p.color_transfer.clone()),
            colorspace: self.probe.as_ref().and_then(|p| p.color_space.clone()),
            mastering_display: self
                .probe
                .as_ref()
                .and_then(|p| p.hdr.mastering_display.clone()),
            max_cll: self.probe.as_ref().and_then(|p| p.hdr.max_cll.clone()),
            ..Default::default()
        };

        match encode_with_x265(self.input, cpu_path, &config, &cpu_vf_args) {
            Ok(_) => Self::read_probe_output_size(cpu_path, "CPU test file metadata (x265)"),
            Err(err) => {
                crate::log_failure!(
                    crate::infra::static_logs::messages::LABEL_DYNAMIC,
                    format!("CPU x265 encoding failed for CRF {anchor_crf:.1}: {err}")
                );
                None
            }
        }
    }

    fn run_cpu_generic_probe(&self, anchor_crf: f32, cpu_path: &Path) -> Option<u64> {
        let mut cpu_builder = crate::ffmpeg_builder::FfmpegBuilder::new();
        cpu_builder
            .overwrite()
            .input(self.input)
            .arg("-map")
            .arg("0:v:0");

        let vf_joined = build_calibration_filter_chain(
            self.vf_args,
            Some(self.input_duration),
            self.ultimate_mode,
            &[],
        );
        if !vf_joined.is_empty() {
            cpu_builder.arg("-vf").arg(vf_joined);
        }
        if self.sampling_mode == CalibrationSamplingMode::Timed {
            cpu_builder.arg("-t").arg(self.sample_duration.to_string());
        }

        cpu_builder
            .codec_video(self.encoder.ffmpeg_name())
            .arg("-crf")
            .arg(format!("{anchor_crf:.1}"));

        for arg in self.encoder.extra_args(self.max_threads, self.apple_compat) {
            cpu_builder.arg(arg);
        }

        cpu_builder.codec_audio("none");
        cpu_builder.output(cpu_path);

        match cpu_builder.build().output() {
            Ok(out) if out.status.success() => {
                Self::read_probe_output_size(cpu_path, "CPU test file metadata (generic)")
            }
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                crate::log_failure!(
                    crate::infra::static_logs::messages::LABEL_DYNAMIC,
                    format!("CPU encoding failed for CRF {anchor_crf:.1}")
                );
                if stderr.contains("No such encoder") {
                    crate::log_failure!(
                        crate::infra::static_logs::messages::LABEL_DYNAMIC,
                        "Cause: CPU encoder not available"
                    );
                }
                None
            }
            Err(err) => {
                crate::log_failure!(
                    crate::infra::static_logs::messages::LABEL_DYNAMIC,
                    format!("CPU command failed: {err}")
                );
                None
            }
        }
    }

    fn read_probe_output_size(path: &Path, label: &str) -> Option<u64> {
        match crate::stream_size::measure_strict_pure_media(path) {
            Ok(measurement) => Some(measurement.pure_media_size()),
            Err(err) => {
                crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                    "dynamic_mapping_pure_media_probe",
                    format!(
                        "{label}: strict pure-media measurement failed for {}: {err}",
                        path.display()
                    ),
                );
                None
            }
        }
    }
}

/// Quickly calibrate a CRF value using a GPU-accelerated coarse search.
///
/// # Errors
/// Returns an error if the search fails.
///
/// # Panics
/// Panics if the input file path is not a valid UTF-8 string.
// Rationale: This function handles complex, sequential initialization or
// business logic where further fragmentation would hinder readability and
// maintainability.
pub fn quick_calibrate(
    input: &Path,
    input_size: u64,
    encoder: super::VideoEncoder,
    vf_args: &[String],
    gpu_encoder: &crate::gpu_accel::GpuEncoder,
    sample_duration: f32,
    ultimate_mode: bool,
    apple_compat: bool,
) -> Result<DynamicCrfMapper> {
    let mut mapper = DynamicCrfMapper::new(input_size);
    let context = CalibrationContext::new(
        input,
        encoder,
        vf_args,
        gpu_encoder,
        sample_duration,
        ultimate_mode,
        apple_compat,
    )?;
    let calibration_crfs = crate::constants::DYNAMIC_MAPPING_CALIBRATION_CRFS;
    let mut calibration_success = false;

    for (attempt, anchor_crf) in calibration_crfs.iter().enumerate() {
        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_DYNAMIC,
            format!(
                "Dynamic calibration attempt {}/{}: Testing CRF {:.1}...",
                attempt + 1,
                calibration_crfs.len(),
                anchor_crf
            )
        );

        let gpu_test_file =
            crate::media_conversion_gate::delivery_named_tempfile_in_scratch_or_err(
                "dynamic_mapping_gpu_cal",
                None,
                Some(".mp4"),
            )
            .context("Failed to create temp file")?;
        let cpu_test_file =
            crate::media_conversion_gate::delivery_named_tempfile_in_scratch_or_err(
                "dynamic_mapping_cpu_cal",
                None,
                Some(".mp4"),
            )
            .context("Failed to create temp file")?;
        let gpu_path = gpu_test_file.path().to_path_buf();
        let cpu_path = cpu_test_file.path().to_path_buf();

        let Some((gpu_size, cpu_size)) = context.try_anchor(*anchor_crf, &gpu_path, &cpu_path)?
        else {
            continue;
        };

        if gpu_size > 0 && cpu_size > 0 {
            mapper.add_anchor(*anchor_crf, gpu_size, cpu_size);

            let ratio = crate::numeric_cast::u64_to_f64(cpu_size)
                / crate::numeric_cast::u64_to_f64(gpu_size);

            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_DYNAMIC,
                format!("Calibration successful at CRF {anchor_crf:.1}")
            );
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_DYNAMIC,
                format!("GPU: {gpu_size} bytes, CPU: {cpu_size} bytes (ratio: {ratio:.2})")
            );
            calibration_success = true;
            break;
        }
    }

    if !calibration_success {
        crate::media_conversion_gate::explore_calibration_degraded_audit(
            "All CPU calibration attempts failed, using static offset",
        );
        crate::media_conversion_gate::explore_calibration_degraded_audit(format!(
            "Tried CRF values: {calibration_crfs:?}"
        ));
        crate::media_conversion_gate::explore_calibration_degraded_audit(
            "This may affect GPU→CPU mapping accuracy",
        );
        return Ok(mapper);
    }

    {
        if let Some(anchor) = mapper.anchors.first() {
            let ratio = crate::numeric_cast::u64_to_f64(anchor.cpu_size)
                / crate::numeric_cast::u64_to_f64(anchor.gpu_size);
            let offset = DynamicCrfMapper::calculate_offset_from_ratio(ratio);
            let gpu_size = anchor.gpu_size;
            let cpu_size = anchor.cpu_size;
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_DYNAMIC,
                format!(
                    "Calibration complete: GPU {gpu_size} → CPU {cpu_size} (ratio {ratio:.3}, \
                     offset +{offset:.1})"
                )
            );
        }
    }

    Ok(mapper)
}

#[cfg(test)]
mod tests {
    use super::{build_calibration_filter_chain, collect_vf_filters};

    #[test]
    fn test_collect_vf_filters_merges_multiple_pairs() {
        let vf_args = vec![
            "-vf".to_string(),
            "scale=1280:720".to_string(),
            "-crf".to_string(),
            "20".to_string(),
            "-vf".to_string(),
            "fps=30".to_string(),
        ];

        assert_eq!(
            collect_vf_filters(&vf_args),
            vec!["scale=1280:720".to_string(), "fps=30".to_string()]
        );
    }

    #[test]
    fn test_build_calibration_filter_chain_appends_y4m_guards() {
        let vf_args = vec!["-vf".to_string(), "zscale=t=bt709".to_string()];

        assert_eq!(
            build_calibration_filter_chain(
                &vf_args,
                None,
                false,
                &[
                    "pad=ceil(iw/2)*2:ceil(ih/2)*2:0:0".to_string(),
                    "format=yuv420p10le".to_string(),
                ],
            ),
            "zscale=t=bt709,pad=ceil(iw/2)*2:ceil(ih/2)*2:0:0,format=yuv420p10le"
        );
    }

    #[test]
    fn test_build_calibration_filter_chain_without_input_filters() {
        assert_eq!(
            build_calibration_filter_chain(
                &[],
                None,
                false,
                &[
                    "pad=ceil(iw/2)*2:ceil(ih/2)*2:0:0".to_string(),
                    "format=yuv420p".to_string(),
                ],
            ),
            "pad=ceil(iw/2)*2:ceil(ih/2)*2:0:0,format=yuv420p"
        );
    }

    #[test]
    fn test_build_calibration_filter_chain_adds_sampling_prefix_for_long_videos() {
        let vf_args = vec!["-vf".to_string(), "format=yuv420p".to_string()];

        let filter = build_calibration_filter_chain(
            &vf_args,
            Some(120.0_f64),
            false,
            &["pad=ceil(iw/2)*\
                                                                                2:ceil(ih/2)*2:\
                                                                                0:0"
            .to_string()],
        );

        assert!(filter.starts_with("select='between(t,0.0,15.0)"));
        assert!(filter.contains(",format=yuv420p,pad=ceil(iw/2)*2:ceil(ih/2)*2:0:0"));
    }

    #[test]
    fn test_build_calibration_filter_chain_omits_sampling_prefix_for_short_videos() {
        let vf_args = vec!["-vf".to_string(), "scale=1280:720".to_string()];

        assert_eq!(
            build_calibration_filter_chain(&vf_args, Some(10.0_f64), false, &[]),
            "scale=1280:720"
        );
    }

    #[test]
    fn test_dynamic_crf_mapper_gpu_to_cpu() {
        use crate::video_explorer::dynamic_mapping::DynamicCrfMapper;
        let mut mapper = DynamicCrfMapper::new(1000);

        // No anchors: should use base_offset
        let (cpu, conf) = mapper.gpu_to_cpu(20.0, 4.0, 51.0);
        assert!((cpu - 24.0).abs() < 1e-6, "cpu mismatch: got {cpu}");
        assert!(conf < 0.6);

        // One anchor: ratio 0.8 (TIER_3) -> offset 3.0
        mapper.add_anchor(20.0, 100, 80);
        let (cpu, _conf) = mapper.gpu_to_cpu(22.0, 4.0, 51.0);
        assert!((cpu - 25.0).abs() < 1e-6, "cpu mismatch: got {cpu}");

        // Two anchors: interpolation
        // Anchor 1: CRF 20, ratio 0.8 -> offset 3.0
        // Anchor 2: CRF 30, ratio 0.5 (< 0.7) -> offset 4.0 (TIER_1)
        // At CRF 25, t=0.5, interpolated offset = 3.5
        mapper.add_anchor(30.0, 100, 50);
        let (cpu, _conf) = mapper.gpu_to_cpu(25.0, 4.0, 51.0);
        assert!((cpu - 28.5).abs() < 1e-6, "cpu mismatch: got {cpu}");
    }
}
