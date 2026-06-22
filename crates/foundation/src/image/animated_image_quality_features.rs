// Animated Image Quality Feature Extractor
//
// Feature extractor for the `animated_image_quality` scenario:
// - GIF / APNG / animated WebP / animated AVIF / animated HEIC / animated JXL
// - strict rejection of static single-frame assets
// - container-agnostic timing and reference-frame quality features

use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Copy, Default)]
pub struct AnimatedImageContentFlags {
    pub is_meme_suspected: bool,
    pub is_seamless_loop: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct AnimatedImageRenderFlags {
    pub has_alpha: bool,
    pub is_lossless: bool,
}

#[derive(Debug, Clone)]
pub struct AnimatedImageQualityFeatures {
    pub format: crate::image_detection::DetectedFormat,
    pub width: u32,
    pub height: u32,
    pub frame_count: u32,
    pub duration_secs: f64,
    pub fps: f64,

    // Palette metrics
    pub palette_size: Option<u16>,
    pub palette_depth: Option<f64>,
    pub color_richness: f64,

    // Animation metrics
    pub average_frame_delay_ms: f64,
    pub frame_delay_variation: f64,
    pub animation_smoothness: f64,
    pub temporal_flicker: f64,

    // Size metrics
    pub file_size_bytes: u64,
    pub bytes_per_pixel: f64,
    pub compression_ratio: f64,

    // Quality indicators
    pub content_flags: AnimatedImageContentFlags,
    pub animation_intensity: f64,
    pub render_flags: AnimatedImageRenderFlags,
    /// Reference-frame entropy when measured; `None` when absent (never fabricated as `0.0`).
    pub reference_entropy: Option<f64>,

    // Physical signal (225D)
    pub physics_225: Vec<f32>,
}

impl AnimatedImageQualityFeatures {
    /// Extract features from an animated image using container-aware analysis.
    ///
    /// # Errors
    /// Returns an error if the path is not an animated image container, timing
    /// metadata is incomplete, reference analysis fails, or derived physics are
    /// invalid.
    pub fn from_path<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        let format = crate::image_detection::detect_format_from_bytes(path)
            .context("Failed to detect animated image format")?;
        anyhow::ensure!(
            supports_animated_image_quality(&format),
            "animated_image_quality only accepts animated image containers (got {})",
            format.as_str()
        );
        let (is_animated, detected_frame_count, detected_fps) =
            crate::image_detection::detect_animation(path, &format)
                .context("Failed to detect animated image timing")?;
        anyhow::ensure!(
            is_animated,
            "animated_image_quality requires a real multi-frame animated image; static assets belong to image_quality"
        );

        let metadata = fs::metadata(path).context("Failed to read animated image file metadata")?;
        let file_size = metadata.len();
        anyhow::ensure!(file_size > 0, "Animated image file is empty");
        let analysis = crate::image_analyzer::analyze_image(path)
            .context("Failed to analyze animated image reference frame")?;

        let probe = crate::ffprobe::probe_video(path)?;
        let width = probe.width.max(analysis.width);
        let height = probe.height.max(analysis.height);
        anyhow::ensure!(
            width > 0 && height > 0,
            "Animated image dimensions must be present and non-zero"
        );

        let gif_timing = if matches!(&format, crate::image_detection::DetectedFormat::GIF) {
            crate::image_formats::gif::get_timing_stats(path).with_context(|| {
                format!("Failed to read GIF timing stats for {}", path.display())
            })?
        } else {
            None
        };
        let frame_count = crate::media_conversion_gate::probe_animated_frame_count_u32(
            probe.frame_count,
            detected_frame_count,
            gif_timing.map(|stats| stats.frame_count),
        )
        .context("Animated image frame count unavailable from ffprobe and native detectors")?;

        let fps_hint = crate::media_conversion_gate::probe_ffprobe_fps_with_detected(
            probe.avg_frame_rate,
            probe.frame_rate,
            detected_fps,
        );
        let duration_secs = crate::media_conversion_gate::probe_animated_duration_secs(
            gif_timing.map(|stats| stats.duration_secs),
            analysis.duration_secs,
            probe.duration,
            frame_count,
            fps_hint,
        )
        .context("Animated image duration unavailable from bitstream timing or ffprobe metadata")?;

        let fps = crate::media_conversion_gate::probe_animated_fps(
            gif_timing.map(|stats| stats.fps),
            probe.avg_frame_rate,
            probe.frame_rate,
            detected_fps,
            frame_count,
            duration_secs,
        )
        .context("Animated image FPS unavailable from bitstream timing or ffprobe metadata")?;

        let palette_size = extract_palette_size(&analysis)?;
        let palette_depth = palette_size.map(estimate_palette_depth);
        let reference_entropy = analysis.features.entropy.and_then(sanitize_positive_f64);
        let color_richness =
            crate::media_conversion_gate::probe_animated_color_richness_unit_interval(
                palette_size,
                reference_entropy,
            );

        let average_frame_delay_ms =
            crate::media_conversion_gate::probe_animated_average_frame_delay_ms(
                gif_timing.map(|stats| stats.average_delay_ms),
                &probe.pts_deltas,
                frame_count,
                duration_secs,
            )
            .context("Animated image average frame delay unavailable from timing metadata")?;

        let frame_delay_variation =
            crate::media_conversion_gate::animated_delay_variation_or_default(
                probe.is_variable_frame_rate,
                crate::media_conversion_gate::probe_animated_timing_variation_or_pts(
                    gif_timing.map(|stats| stats.frame_delay_variation),
                    &probe.pts_deltas,
                ),
                "animated frame_delay_variation",
            );

        let animation_smoothness =
            unit_interval(fps / 30.0) * (1.0 - unit_interval(frame_delay_variation));
        let temporal_flicker = unit_interval(
            (1.0 - unit_interval(average_frame_delay_ms / 120.0))
                .mul_add(0.6, frame_delay_variation * 0.4),
        );

        let bytes_per_pixel =
            crate::numeric_cast::u64_to_f64(file_size) / (f64::from(width) * f64::from(height));
        let compression_ratio = crate::media_conversion_gate::probe_compression_ratio_or_estimate(
            analysis
                .features
                .compression_ratio
                .and_then(sanitize_positive_f64),
            estimate_compression_ratio(bytes_per_pixel, frame_count),
            "animated_image_quality compression_ratio",
        );

        let is_meme_suspected = is_likely_meme(width, height, frame_count, fps);
        let is_seamless_loop = probe.loop_count.is_some_and(|count| count == 0)
            || (frame_delay_variation < 0.3 && frame_count > 2);

        let animation_intensity =
            unit_interval(f64::from(frame_count) / 30.0) * animation_smoothness;
        let physics_225 = extract_reference_frame_physics(path, &analysis)?;

        Ok(Self {
            format,
            width,
            height,
            frame_count,
            duration_secs,
            fps,
            palette_size,
            palette_depth,
            color_richness,
            average_frame_delay_ms,
            frame_delay_variation,
            animation_smoothness,
            temporal_flicker,
            file_size_bytes: file_size,
            bytes_per_pixel,
            compression_ratio,
            content_flags: AnimatedImageContentFlags {
                is_meme_suspected,
                is_seamless_loop,
            },
            animation_intensity,
            render_flags: AnimatedImageRenderFlags {
                has_alpha: analysis.has_alpha,
                is_lossless: analysis.is_lossless,
            },
            reference_entropy,
            physics_225,
        })
    }

    /// Convert features to a normalized vector for embedding
    #[must_use]
    pub fn to_embedding_vector(&self) -> Vec<f32> {
        let mut vec = vec![0.0_f32; 256];

        crate::real_physics::encode_normalized_physics_225(&mut vec, 0, &self.physics_225);

        let pixel_count = (f64::from(self.width) * f64::from(self.height)).max(1.0);
        let aspect_ratio = f64::from(self.width) / f64::from(self.height.max(1));
        let bytes_per_frame = crate::numeric_cast::u64_to_f64(self.file_size_bytes)
            / f64::from(self.frame_count.max(1));
        let bits_per_pixel_frame = (crate::numeric_cast::u64_to_f64(self.file_size_bytes) * 8.0)
            / (pixel_count * f64::from(self.frame_count.max(1)));
        let cadence_consistency = 1.0 - unit_interval(self.frame_delay_variation);
        let short_loop_bias = 1.0 - unit_interval(self.duration_secs / 15.0);
        let high_fps_flag = if self.fps >= 24.0 { 1.0 } else { 0.0 };
        let low_fps_flag = if self.fps <= 12.0 { 1.0 } else { 0.0 };
        let squareish_flag = if (0.8..=1.25).contains(&aspect_ratio) {
            1.0
        } else {
            0.0
        };
        let palette_pressure = match self.palette_depth {
            Some(depth) => {
                let v = unit_interval(self.color_richness * (depth / 8.0));
                if v.is_finite() { v } else { f64::NAN }
            }
            None => f64::NAN,
        };
        let flicker_intensity = unit_interval(self.temporal_flicker * self.animation_intensity);

        vec[225] = unit_interval_f32(crate::numeric_cast::u32_to_f32(self.width) / 4096.0);
        vec[226] = unit_interval_f32(crate::numeric_cast::u32_to_f32(self.height) / 4096.0);
        vec[227] =
            unit_interval_f32(nonnegative_f32(f64_to_f32_feature(pixel_count.log10())) / 8.0);
        vec[228] =
            unit_interval_f32(nonnegative_f32(f64_to_f32_feature(aspect_ratio)).ln_1p() / 2.5);
        vec[229] = unit_interval_f32(crate::numeric_cast::u32_to_f32(self.frame_count) / 1000.0);
        vec[230] = unit_interval_f32(f64_to_f32_feature(self.duration_secs) / 60.0);
        vec[231] = unit_interval_f32(f64_to_f32_feature(self.fps) / 60.0);
        vec[232] = unit_interval_f32(f64_to_f32_feature(self.average_frame_delay_ms) / 500.0);
        vec[233] = unit_interval_f32(f64_to_f32_feature(self.frame_delay_variation));
        vec[234] = unit_interval_f32(f64_to_f32_feature(self.animation_smoothness));
        vec[235] = unit_interval_f32(f64_to_f32_feature(self.temporal_flicker));
        vec[236] = unit_interval_f32(
            nonnegative_f32(f64_to_f32_feature(
                crate::numeric_cast::u64_to_f64(self.file_size_bytes).log10(),
            )) / 10.0,
        );
        vec[237] = unit_interval_f32(f64_to_f32_feature(self.bytes_per_pixel) / 10.0);
        vec[238] = unit_interval_f32(f64_to_f32_feature(self.compression_ratio));
        vec[239] = crate::media_conversion_gate::quality_embedding_optional_unit_interval_f32(
            self.palette_size
                .map(|size| f64::from(unit_interval_f32(f32::from(size) / 256.0))),
        );
        vec[240] = crate::media_conversion_gate::quality_embedding_optional_unit_interval_f32(
            self.palette_depth
                .map(|depth| f64::from(unit_interval_f32(f64_to_f32_feature(depth) / 8.0))),
        );
        vec[241] = unit_interval_f32(f64_to_f32_feature(self.color_richness));
        vec[242] = if self.content_flags.is_meme_suspected {
            1.0
        } else {
            0.0
        };
        vec[243] = if self.content_flags.is_seamless_loop {
            1.0
        } else {
            0.0
        };
        vec[244] = unit_interval_f32(f64_to_f32_feature(self.animation_intensity));
        vec[245] =
            unit_interval_f32(nonnegative_f32(f64_to_f32_feature(bytes_per_frame.ln_1p())) / 14.0);
        vec[246] = unit_interval_f32(
            nonnegative_f32(f64_to_f32_feature(bits_per_pixel_frame.ln_1p())) / 4.0,
        );
        vec[247] = if self.render_flags.has_alpha {
            1.0
        } else {
            0.0
        };
        vec[248] = unit_interval_f32(f64_to_f32_feature(cadence_consistency));
        vec[249] = unit_interval_f32(f64_to_f32_feature(short_loop_bias));
        vec[250] = high_fps_flag;
        vec[251] = low_fps_flag;
        vec[252] = squareish_flag;
        vec[253] = match (self.reference_entropy, palette_pressure.is_finite()) {
            (Some(entropy), true) => unit_interval_f32(f64_to_f32_feature(
                palette_pressure * unit_interval(entropy / 8.0),
            )),
            _ => f32::NAN,
        };
        vec[254] = unit_interval_f32(f64_to_f32_feature(flicker_intensity));
        vec[255] = if self.render_flags.is_lossless {
            1.0
        } else {
            0.0
        };

        debug_assert!(
            vec.iter()
                .enumerate()
                .all(|(idx, v)| v.is_finite() || animated_embed_absent_slot(idx)),
            "animated embedding: non-finite only on absent-measurement slots (239/240/253)"
        );

        vec
    }
}

const fn supports_animated_image_quality(format: &crate::image_detection::DetectedFormat) -> bool {
    matches!(
        format,
        crate::image_detection::DetectedFormat::GIF
            | crate::image_detection::DetectedFormat::PNG
            | crate::image_detection::DetectedFormat::WebP
            | crate::image_detection::DetectedFormat::HEIC
            | crate::image_detection::DetectedFormat::HEIF
            | crate::image_detection::DetectedFormat::AVIF
            | crate::image_detection::DetectedFormat::JXL
    )
}

fn extract_reference_frame_physics(
    path: &Path,
    analysis: &crate::image_analyzer::ImageAnalysis,
) -> Result<Vec<f32>> {
    if let Some(physics) = &analysis.physics_225 {
        return validate_physics_225(physics.clone(), "animated image reference physics");
    }

    let img = image::ImageReader::open(path)
        .context("Failed to open animated image reference frame")?
        .decode()
        .context("Failed to decode animated image reference frame")?
        .0;
    validate_physics_225(
        crate::real_physics::extract_image_physics_225(&img),
        "animated image decoded reference physics",
    )
}

fn validate_physics_225(physics: Vec<f32>, context: &str) -> Result<Vec<f32>> {
    if physics.len() != 225 {
        anyhow::bail!("{context} length {} != 225", physics.len());
    }
    if physics.iter().any(|value| !value.is_finite()) {
        anyhow::bail!("{context} contains non-finite values");
    }
    if physics.iter().all(|value| value.abs() <= f32::EPSILON) {
        anyhow::bail!("{context} is all zero");
    }
    Ok(physics)
}

fn extract_palette_size(analysis: &crate::image_analyzer::ImageAnalysis) -> Result<Option<u16>> {
    let palette_size = analysis
        .precision
        .palette_size
        .map(|size| u16::try_from(size).context("Animated-image palette size exceeds u16"))
        .transpose()?;
    Ok(palette_size)
}

/// Estimate palette depth (bits per pixel)
const fn estimate_palette_depth(palette_size: u16) -> f64 {
    if palette_size <= 2 {
        1.0
    } else if palette_size <= 4 {
        2.0
    } else if palette_size <= 16 {
        4.0
    } else {
        8.0
    }
}

/// Estimate compression ratio based on bytes per pixel
fn estimate_compression_ratio(bytes_per_pixel: f64, frame_count: u32) -> f64 {
    if frame_count == 0 || !bytes_per_pixel.is_finite() {
        return 0.01;
    }

    // Uncompressed animated image would be ~3 bytes per pixel (RGB) per frame
    let expected_uncompressed = 3.0 * f64::from(frame_count);
    unit_interval(bytes_per_pixel / expected_uncompressed).max(0.01)
}

/// Heuristic for meme detection
fn is_likely_meme(width: u32, height: u32, frame_count: u32, fps: f64) -> bool {
    // Memes typically:
    // - Are roughly square or wide (aspect ratio ~1:1 to 2:1)
    // - Have 1-20 frames
    // - Play at 5-10 FPS

    let aspect = f64::from(width) / f64::from(height.max(1));
    let is_square_ish = (0.5..=2.0).contains(&aspect);
    let is_short = (1..=20).contains(&frame_count);
    let is_slow = (5.0..=15.0).contains(&fps);

    is_square_ish && is_short && is_slow
}

fn sanitize_positive_f64(value: f64) -> Option<f64> {
    crate::media_conversion_gate::probe_positive_f64(value)
}

const fn unit_interval(value: f64) -> f64 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// Animated tail embed slots that may be `NaN` when the source `Option` is absent.
const fn animated_embed_absent_slot(index: usize) -> bool {
    matches!(index, 239 | 240 | 253)
}

const fn unit_interval_f32(value: f32) -> f32 {
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

const fn f64_to_f32_feature(value: f64) -> f32 {
    crate::numeric_cast::f64_to_f32_lossy(value)
}

#[cfg(test)]
mod smoke_tests {
    use super::*;

    fn parse_fps(s: &str) -> Result<f64, crate::ffprobe::FFprobeError> {
        crate::ffprobe::parse_frame_rate(s)
    }

    #[test]
    fn smoke_parse_fps() {
        let fps = parse_fps("30/1").unwrap_or_else(|err| panic!("parse failed: {err}"));
        assert!((fps - 30.0).abs() < 0.01);
        let fps = parse_fps("29.97").unwrap_or_else(|err| panic!("parse failed: {err}"));
        assert!((fps - 29.97).abs() < 0.01);
        assert!(parse_fps("invalid").is_err());
    }

    #[test]
    fn smoke_palette_depth() {
        assert!((estimate_palette_depth(2) - 1.0).abs() < f64::EPSILON);
        assert!((estimate_palette_depth(16) - 4.0).abs() < f64::EPSILON);
        assert!((estimate_palette_depth(256) - 8.0).abs() < f64::EPSILON);
    }

    #[test]
    fn smoke_embedding_vector_layout() {
        let features = AnimatedImageQualityFeatures {
            format: crate::image_detection::DetectedFormat::GIF,
            width: 256,
            height: 256,
            frame_count: 10,
            duration_secs: 1.0,
            fps: 10.0,
            palette_size: Some(256),
            palette_depth: Some(8.0),
            color_richness: 1.0,
            average_frame_delay_ms: 100.0,
            frame_delay_variation: 0.0,
            animation_smoothness: 1.0,
            temporal_flicker: 0.2,
            file_size_bytes: 10000,
            bytes_per_pixel: 0.15,
            compression_ratio: 0.5,
            content_flags: AnimatedImageContentFlags {
                is_meme_suspected: false,
                is_seamless_loop: true,
            },
            animation_intensity: 0.8,
            render_flags: AnimatedImageRenderFlags {
                has_alpha: false,
                is_lossless: true,
            },
            reference_entropy: Some(7.0),
            physics_225: vec![0.5; 225],
        };

        let vec = features.to_embedding_vector();
        assert_eq!(vec.len(), 256);
        assert!(
            vec.iter()
                .enumerate()
                .all(|(idx, v)| v.is_finite() || animated_embed_absent_slot(idx))
        );
        assert!(vec.iter().all(|v| (0.0..=1.0).contains(v)));
        assert!(vec[0] > 0.4);
        assert!((vec[240] - 1.0).abs() < f32::EPSILON);
        assert!((vec[255] - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn smoke_embedding_vector_sanitizes_invalid_metadata() {
        let features = AnimatedImageQualityFeatures {
            format: crate::image_detection::DetectedFormat::WebP,
            width: u32::MAX,
            height: u32::MAX,
            frame_count: u32::MAX,
            duration_secs: f64::NAN,
            fps: f64::INFINITY,
            palette_size: Some(256),
            palette_depth: Some(f64::NAN),
            color_richness: f64::INFINITY,
            average_frame_delay_ms: f64::NEG_INFINITY,
            frame_delay_variation: f64::NAN,
            animation_smoothness: f64::INFINITY,
            temporal_flicker: f64::NEG_INFINITY,
            file_size_bytes: 0,
            bytes_per_pixel: f64::INFINITY,
            compression_ratio: f64::NAN,
            content_flags: AnimatedImageContentFlags {
                is_meme_suspected: false,
                is_seamless_loop: false,
            },
            animation_intensity: f64::INFINITY,
            render_flags: AnimatedImageRenderFlags {
                has_alpha: true,
                is_lossless: false,
            },
            reference_entropy: None,
            physics_225: vec![f32::INFINITY; 225],
        };

        let vec = features.to_embedding_vector();
        assert_eq!(vec.len(), 256);
        assert!(
            vec[253].is_nan(),
            "missing entropy must not become 0.0 in embed slot 253"
        );
        assert!(
            vec.iter()
                .enumerate()
                .all(|(idx, v)| v.is_finite() || animated_embed_absent_slot(idx))
        );
    }

    #[test]
    fn smoke_embedding_vector_normalizes_signed_physics_tail() {
        let mut physics = vec![0.0_f32; 225];
        physics[0] = 0.25;
        physics[1] = 0.25;
        physics[2] = -10.0;
        physics[3] = 10.0;
        physics[24] = 0.0;

        let features = AnimatedImageQualityFeatures {
            format: crate::image_detection::DetectedFormat::GIF,
            width: 256,
            height: 256,
            frame_count: 10,
            duration_secs: 1.0,
            fps: 10.0,
            palette_size: Some(256),
            palette_depth: Some(8.0),
            color_richness: 1.0,
            average_frame_delay_ms: 100.0,
            frame_delay_variation: 0.0,
            animation_smoothness: 1.0,
            temporal_flicker: 0.2,
            file_size_bytes: 10000,
            bytes_per_pixel: 0.15,
            compression_ratio: 0.5,
            content_flags: AnimatedImageContentFlags {
                is_meme_suspected: false,
                is_seamless_loop: true,
            },
            animation_intensity: 0.8,
            render_flags: AnimatedImageRenderFlags {
                has_alpha: false,
                is_lossless: true,
            },
            reference_entropy: Some(7.0),
            physics_225: physics,
        };

        let vec = features.to_embedding_vector();
        assert!((vec[0] - 0.25).abs() < 1.0e-6);
        assert!((vec[1] - 1.0).abs() < 1.0e-6);
        assert!((vec[2] - 0.0).abs() < 1.0e-6);
        assert!((vec[3] - 1.0).abs() < 1.0e-6);
        assert!((vec[24] - 0.5).abs() < 1.0e-6);
    }
}
