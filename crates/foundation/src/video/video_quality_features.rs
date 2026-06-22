// Video Quality Feature Extractor (Multi-Scenario 256D)
//
// Extracts dense features for video quality assessment:
// - [0-224]: Physical signal from representative center frame (Color, DCT, HOG)
// - [225-255]: Dense video-specific metrics (codec, bitrate, cadence, perceptual evidence)

use crate::builder_base::ToolBuilder;
use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct VideoQualityFeatures {
    pub width: u32,
    pub height: u32,
    pub duration_secs: f64,
    pub fps: f64,
    pub frame_count: u64,

    // Codec metrics
    pub codec: String,
    pub bitrate_mbps: f64,
    pub file_size_bytes: u64,
    pub bit_depth: Option<u8>,
    pub has_audio: bool,
    pub is_variable_frame_rate: bool,
    pub is_hdr: bool,

    // Temporal characteristics
    pub motion_intensity: f64,
    pub temporal_stability: f64,

    // Physical signal (225D)
    pub physics_225: Option<Vec<f32>>,
}

impl VideoQualityFeatures {
    /// Extract features from a real video container.
    ///
    /// # Errors
    /// Returns an error if the path points to an animated image, metadata is
    /// incomplete, frame extraction fails, or the sampled frame cannot be
    /// decoded.
    pub fn from_path<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        let format = crate::image_detection::detect_format_from_bytes(path).with_context(|| {
            format!(
                "Video quality true-format detection failed: {}",
                path.display()
            )
        })?;
        if matches!(
            &format,
            crate::image_detection::DetectedFormat::GIF
                | crate::image_detection::DetectedFormat::PNG
                | crate::image_detection::DetectedFormat::WebP
                | crate::image_detection::DetectedFormat::HEIC
                | crate::image_detection::DetectedFormat::HEIF
                | crate::image_detection::DetectedFormat::AVIF
                | crate::image_detection::DetectedFormat::JXL
        ) {
            let (is_animated, _, _) = crate::image_detection::detect_animation(path, &format)?;
            if is_animated {
                anyhow::bail!(
                    "video_quality only accepts real video containers; animated images belong to animated_image_quality"
                );
            }
            anyhow::bail!(
                "video_quality only accepts real video containers; static images belong to image_quality"
            );
        }

        let metadata = fs::metadata(path).context("Video FS access failed")?;
        let file_size = metadata.len();
        anyhow::ensure!(file_size > 0, "Video file is empty");

        let probe = crate::ffprobe::probe_video(path).context("FFprobe failed")?;
        let width = probe.width;
        let height = probe.height;
        anyhow::ensure!(
            width > 0 && height > 0,
            "Video dimensions must be present and non-zero"
        );
        let codec = probe.video_codec.trim();
        anyhow::ensure!(
            !codec.is_empty() && !codec.eq_ignore_ascii_case("unknown"),
            "Video codec must be known for video_quality ingestion"
        );

        let probe_frame_count = probe.frame_count.filter(|count| *count > 0);
        let probe_fps = crate::media_conversion_gate::probe_ffprobe_fps_avg_or_r_frame_rate(
            probe.avg_frame_rate,
            probe.frame_rate,
        );
        let duration_secs = crate::media_conversion_gate::probe_video_quality_duration_secs(
            probe.duration,
            probe_frame_count,
            probe_fps,
        )
        .context("Video duration unavailable from ffprobe metadata")?;
        let fps = crate::media_conversion_gate::probe_video_quality_fps(
            probe.avg_frame_rate,
            probe.frame_rate,
            probe_frame_count,
            duration_secs,
        )
        .context("Video FPS unavailable from ffprobe metadata")?;
        let frame_count = crate::media_conversion_gate::probe_video_quality_frame_count(
            probe_frame_count,
            duration_secs,
            Some(fps),
        )
        .context("Video frame count unavailable from ffprobe metadata")?;

        let bitrate_mbps = crate::media_conversion_gate::probe_video_quality_bitrate_mbps(
            probe.video_bit_rate,
            probe.bit_rate,
            file_size,
            duration_secs,
        )
        .context("Video bitrate unavailable from metadata and file-size derivation")?;

        let physics_225 = Some(extract_center_frame_physics(path, duration_secs / 2.0)?);

        // Motion Intensity (Using bitrate vs resolution vs duration heuristic)
        let motion_intensity = if duration_secs > 0.1 {
            let res_factor = (crate::numeric_cast::u32_to_f64(width)
                * crate::numeric_cast::u32_to_f64(height))
                / 1_000_000.0;
            (bitrate_mbps / res_factor.max(0.1) / 10.0).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let temporal_stability = estimate_temporal_stability(duration_secs, fps, frame_count);

        Ok(Self {
            width,
            height,
            duration_secs,
            fps,
            frame_count,
            codec: codec.to_string(),
            bitrate_mbps,
            file_size_bytes: file_size,
            bit_depth: probe.bit_depth,
            has_audio: probe.audio.present,
            is_variable_frame_rate: probe.is_variable_frame_rate,
            is_hdr: probe.is_hdr(),
            motion_intensity,
            temporal_stability,
            physics_225,
        })
    }

    #[must_use]
    pub fn to_embedding_vector(&self) -> Vec<f32> {
        let mut vec = vec![0.0_f32; 256];

        // 1. Physical Signal [0-224]
        if let Some(physics) = &self.physics_225 {
            crate::real_physics::encode_normalized_physics_225(&mut vec, 0, physics);
        }

        // 2. One-hot Codec [225-229]
        let c = self.codec.to_lowercase();
        if c.contains("h264") || c.contains("avc") {
            vec[225] = 1.0;
        } else if c.contains("h265") || c.contains("hevc") {
            vec[226] = 1.0;
        } else if c.contains("vp9") {
            vec[227] = 1.0;
        } else if c.contains("av1") {
            vec[228] = 1.0;
        } else {
            vec[229] = 1.0;
        } // Other

        // 3. Dense Video Metrics [230-255]
        vec[230] = unit_interval(f64_to_f32_feature(self.bitrate_mbps) / 20.0);
        vec[231] = unit_interval(f64_to_f32_feature(self.fps) / 60.0);
        let pixel_count = (crate::numeric_cast::u32_to_f32(self.width)
            * crate::numeric_cast::u32_to_f32(self.height))
        .max(1.0);
        vec[232] = unit_interval(pixel_count.log10() / 8.0);
        vec[233] = unit_interval(f64_to_f32_feature(self.motion_intensity));
        vec[234] = unit_interval(f64_to_f32_feature(self.temporal_stability));
        vec[235] = unit_interval(u64_to_f32_feature(self.frame_count) / 10_000.0);
        vec[236] = unit_interval(f64_to_f32_feature(self.duration_secs) / 300.0);
        vec[237] = unit_interval(
            nonnegative_f32(f64_to_f32_feature(
                crate::numeric_cast::u64_to_f64(self.file_size_bytes).log10(),
            )) / 10.0,
        );
        let bytes_per_frame =
            u64_to_f32_feature(self.file_size_bytes) / u64_to_f32_feature(self.frame_count.max(1));
        vec[238] = unit_interval(nonnegative_f32(bytes_per_frame.ln_1p()) / 14.0);
        let bits_per_pixel_frame = (u64_to_f32_feature(self.file_size_bytes) * 8.0)
            / (pixel_count * u64_to_f32_feature(self.frame_count.max(1)));
        vec[239] = unit_interval(nonnegative_f32(bits_per_pixel_frame.ln_1p()) / 4.0);
        let aspect_ratio = crate::numeric_cast::u32_to_f32(self.width)
            / crate::numeric_cast::u32_to_f32(self.height.max(1));
        vec[240] = unit_interval(nonnegative_f32(aspect_ratio).ln_1p() / 2.5);
        vec[241] = unit_interval(crate::numeric_cast::u32_to_f32(self.width) / 8192.0);
        vec[242] = unit_interval(crate::numeric_cast::u32_to_f32(self.height) / 8192.0);
        vec[243] = crate::media_conversion_gate::quality_embedding_optional_unit_interval_f32(
            self.bit_depth
                .map(|bit_depth| f64::from(unit_interval(f32::from(bit_depth) / 16.0))),
        );
        vec[244] = if self.has_audio { 1.0 } else { 0.0 };
        vec[245] = if self.is_variable_frame_rate {
            1.0
        } else {
            0.0
        };
        vec[246] = if self.is_hdr { 1.0 } else { 0.0 };
        let megapixels = (pixel_count / 1_000_000.0).max(0.1);
        vec[247] = unit_interval((megapixels * f64_to_f32_feature(self.fps)) / 240.0);

        let codec_factor: f32 = if vec[226] > 0.5 || vec[228] > 0.5 {
            1.5
        } else {
            1.0
        };
        vec[248] =
            unit_interval(codec_factor * 5.0 / f64_to_f32_feature(self.bitrate_mbps.max(0.1)));
        let expected_frames = (self.duration_secs * self.fps).max(1.0);
        let cadence_error = ((crate::numeric_cast::u64_to_f64(self.frame_count) - expected_frames)
            .abs()
            / expected_frames)
            .min(1.0);
        vec[249] = unit_interval(f64_to_f32_feature(1.0 - cadence_error));
        vec[250] = unit_interval(f64_to_f32_feature(
            self.motion_intensity * (1.0 - self.temporal_stability),
        ));
        let pixels_mpix = (pixel_count / 1_000_000.0).max(0.1);
        vec[251] = unit_interval((f64_to_f32_feature(self.bitrate_mbps) / pixels_mpix) / 50.0);
        let avg_frame_duration_ms = if self.frame_count > 0 {
            (f64_to_f32_feature(self.duration_secs) * 1000.0) / u64_to_f32_feature(self.frame_count)
        } else {
            0.0
        };
        vec[252] = unit_interval(avg_frame_duration_ms / 200.0);
        vec[253] = if self.fps >= 50.0 { 1.0 } else { 0.0 };
        vec[254] = if self.duration_secs >= 30.0 { 1.0 } else { 0.0 };
        let quality_blend = (vec[233] + vec[234] + vec[243] + (1.0 - vec[248]) + vec[251]) / 5.0;
        vec[255] = unit_interval(quality_blend);

        vec
    }
}

const fn unit_interval(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

const fn nonnegative_f32(value: f32) -> f32 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}

fn estimate_temporal_stability(duration_secs: f64, fps: f64, frame_count: u64) -> f64 {
    if !duration_secs.is_finite()
        || !fps.is_finite()
        || duration_secs <= 0.1
        || fps <= 0.1
        || frame_count == 0
    {
        return 0.5;
    }

    let expected_frames = duration_secs * fps;
    if !expected_frames.is_finite() || expected_frames <= 1.0 {
        return 0.5;
    }

    let cadence_error = ((crate::numeric_cast::u64_to_f64(frame_count) - expected_frames).abs()
        / expected_frames)
        .min(1.0);
    (1.0 - cadence_error).clamp(0.0, 1.0)
}

const fn f64_to_f32_feature(value: f64) -> f32 {
    crate::numeric_cast::f64_to_f32_lossy(value)
}

const fn u64_to_f32_feature(value: u64) -> f32 {
    crate::numeric_cast::f64_to_f32_lossy(crate::numeric_cast::u64_to_f64(value))
}

fn extract_center_frame_physics(path: &Path, center_time: f64) -> Result<Vec<f32>> {
    // 🛡️ PERFORMANCE: Move -ss BEFORE input for fast input-seek
    // Note: input-seek snaps to nearest keyframe; actual frame may differ from center_time
    let output = crate::ffmpeg_builder::FfmpegBuilder::new()
        .arg("-ss")
        .arg(format!("{center_time:.3}"))
        .input(path)
        .frames_v(1)
        .arg("-vf")
        .arg("scale=min(iw\\,512):-1")
        .format("image2")
        .arg("-vcodec")
        .arg("mjpeg")
        .output_pipe()
        .build()
        .output()?;

    if !output.status.success() || output.stdout.is_empty() {
        anyhow::bail!("FFmpeg frame extraction failed");
    }

    let img = image::load_from_memory_with_format(&output.stdout, image::ImageFormat::Jpeg)
        .context("Failed to decode sampled frame")?;

    Ok(crate::real_physics::extract_image_physics_225(&img))
}

/// # Errors
/// Returns an error if the frame-rate fraction cannot be parsed.
pub fn parse_fraction(s: &str) -> Result<f64, crate::ffprobe::FFprobeError> {
    crate::ffprobe::parse_frame_rate(s)
}

#[cfg(test)]
mod smoke_tests {
    use super::*;

    #[test]
    fn smoke_parse_fraction() {
        let fps = parse_fraction("30000/1001").unwrap_or_else(|err| panic!("parse failed: {err}"));
        assert!((fps - 29.97).abs() < 0.01);
        assert!(parse_fraction("invalid").is_err());
    }

    #[test]
    fn smoke_temporal_stability_heuristic() {
        assert!(estimate_temporal_stability(10.0, 30.0, 300) > 0.95);
        assert!(estimate_temporal_stability(10.0, 30.0, 120) < 0.5);
        assert!((estimate_temporal_stability(0.0, 30.0, 300) - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn smoke_embedding_vector_layout() {
        // Mock a feature set
        let features = VideoQualityFeatures {
            width: 1920,
            height: 1080,
            duration_secs: 10.0,
            fps: 30.0,
            frame_count: 300,
            codec: "h264".to_string(),
            bitrate_mbps: 5.0,
            file_size_bytes: 5_000_000,
            bit_depth: Some(10),
            has_audio: true,
            is_variable_frame_rate: false,
            is_hdr: false,
            motion_intensity: 0.5,
            temporal_stability: 0.8,
            physics_225: Some(vec![0.5; 225]),
        };

        let vec = features.to_embedding_vector();
        assert_eq!(vec.len(), 256);
        assert!(vec[0] > 0.4); // Physics intact
        assert!(vec[225] > 0.9); // H264 one-hot intact
        assert!(vec[243] > 0.6); // bit depth intact
        assert!(vec[244] > 0.9); // audio presence intact
    }

    #[test]
    fn smoke_embedding_vector_sanitizes_invalid_metadata() {
        let features = VideoQualityFeatures {
            width: 0,
            height: 0,
            duration_secs: f64::NAN,
            fps: f64::INFINITY,
            frame_count: 0,
            codec: "unknown".to_string(),
            bitrate_mbps: 0.0,
            file_size_bytes: 0,
            bit_depth: Some(u8::MAX),
            has_audio: false,
            is_variable_frame_rate: true,
            is_hdr: true,
            motion_intensity: f64::INFINITY,
            temporal_stability: f64::NEG_INFINITY,
            physics_225: None,
        };

        let vec = features.to_embedding_vector();
        assert_eq!(vec.len(), 256);
        assert!(vec.iter().all(|v| v.is_finite()));
        assert!(vec.iter().all(|v| (0.0..=1.0).contains(v)));
    }

    #[test]
    fn smoke_embedding_vector_normalizes_signed_physics_tail() {
        let mut physics = vec![0.0_f32; 225];
        physics[0] = 0.25;
        physics[1] = 0.25;
        physics[2] = -10.0;
        physics[3] = 10.0;
        physics[24] = 0.0;

        let features = VideoQualityFeatures {
            width: 1920,
            height: 1080,
            duration_secs: 10.0,
            fps: 30.0,
            frame_count: 300,
            codec: "h264".to_string(),
            bitrate_mbps: 5.0,
            file_size_bytes: 5_000_000,
            bit_depth: Some(8),
            has_audio: false,
            is_variable_frame_rate: false,
            is_hdr: false,
            motion_intensity: 0.5,
            temporal_stability: 0.8,
            physics_225: Some(physics),
        };

        let vec = features.to_embedding_vector();
        assert!((vec[0] - 0.25).abs() < 1.0e-6);
        assert!((vec[1] - 1.0).abs() < 1.0e-6);
        assert!((vec[2] - 0.0).abs() < 1.0e-6);
        assert!((vec[3] - 1.0).abs() < 1.0e-6);
        assert!((vec[24] - 0.5).abs() < 1.0e-6);
    }
}
