use super::*;
use std::io::Write;
use tempfile::NamedTempFile;

#[test]
fn parse_png_structure_rejects_truncated_text_chunk() {
    let mut data = Vec::new();
    data.extend_from_slice(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]);
    data.extend_from_slice(&13u32.to_be_bytes());
    data.extend_from_slice(b"IHDR");
    data.extend_from_slice(&1u32.to_be_bytes());
    data.extend_from_slice(&1u32.to_be_bytes());
    data.extend_from_slice(&[8, 2, 0, 0, 0]);
    data.extend_from_slice(&0u32.to_be_bytes());
    data.extend_from_slice(&4u32.to_be_bytes());
    data.extend_from_slice(b"tEXt");
    data.extend_from_slice(b"ab");

    match parse_png_structure(std::io::Cursor::new(data)) {
        Ok(_) => panic!("truncated text chunk should fail loudly"),
        Err(err) => assert!(err.to_string().contains("text chunk payload")),
    }
}

#[test]
fn pixel_coordinate_clamp_stays_within_image_bounds() {
    use crate::numeric_cast::u64_to_u32_strict;

    let block_size = 1usize;
    let bx = 2_316_230_111_394_261_264_usize;
    let by = 2_316_230_111_394_261_264_usize;
    let width = 1000u32;
    let height = 1000u32;

    let pixel_x = crate::numeric_cast::usize_to_u64(bx * block_size + block_size / 2);
    let pixel_y = crate::numeric_cast::usize_to_u64(by * block_size + block_size / 2);

    let px = u64_to_u32_strict(pixel_x.min(u64::from(width.saturating_sub(1))), "px")
        .unwrap_or_else(|| {
            unreachable!(
                "CRITICAL: clamped px fits in u32 in analyze_local_entropy simulation (pixel_x={}, width={})",
                pixel_x, width
            )
        });
    let py = u64_to_u32_strict(pixel_y.min(u64::from(height.saturating_sub(1))), "py")
        .unwrap_or_else(|| {
            unreachable!(
                "CRITICAL: clamped py fits in u32 in analyze_local_entropy simulation (pixel_y={}, height={})",
                pixel_y, height
            )
        });

    assert!(px < width);
    assert!(py < height);
    assert_eq!(px, width - 1);
    assert_eq!(py, height - 1);
}

#[test]
fn estimate_lossy_quality_fallback_rejects_invalid_dimensions() {
    let err = estimate_lossy_quality_fallback(
        std::path::Path::new("/tmp/fake-lossy.webp"),
        &DetectedFormat::WebP,
        0,
        1080,
        12345,
        1,
        Some(5.0),
    )
    .err()
    .unwrap_or_else(|| {
        panic!("invalid dimensions should not produce a hardcoded fallback quality")
    });

    match err {
        ImgQualityError::AnalysisError(message) => {
            assert!(message.contains("Cannot estimate quality"));
            assert!(message.contains("invalid dimensions"));
        }
        other => panic!("expected AnalysisError, got {other:?}"),
    }
}

#[test]
fn estimate_lossy_quality_fallback_rejects_missing_entropy() {
    let err = estimate_lossy_quality_fallback(
        std::path::Path::new("/tmp/fake-undecodable.avif"),
        &DetectedFormat::AVIF,
        1920,
        1080,
        500_000,
        1,
        None,
    )
    .err()
    .unwrap_or_else(|| panic!("missing entropy must not produce a synthetic quality verdict"));

    match err {
        ImgQualityError::AnalysisError(message) => {
            assert!(message.contains("entropy unavailable"));
        }
        other => panic!("expected AnalysisError, got {other:?}"),
    }
}

#[test]
fn estimate_lossy_quality_fallback_rejects_zero_entropy() {
    let err = estimate_lossy_quality_fallback(
        std::path::Path::new("/tmp/fake-undecodable-zero.avif"),
        &DetectedFormat::AVIF,
        1920,
        1080,
        500_000,
        1,
        Some(0.0),
    )
    .err()
    .unwrap_or_else(|| {
        unreachable!(
            "CRITICAL: zero entropy must be treated as unmeasured, not as a real reading in test"
        )
    });

    match err {
        ImgQualityError::AnalysisError(message) => {
            assert!(message.contains("entropy unavailable"));
        }
        other => unreachable!("expected AnalysisError, got {other:?} in test"),
    }
}

#[test]
fn detect_format_from_bytes_errors_for_missing_file() {
    let result = detect_format_from_bytes(std::path::Path::new("/nonexistent/file.png"));
    assert!(result.is_err());
}

#[test]
fn color_frequency_distribution_is_zero_for_uniform_image() {
    let uniform_img = image::DynamicImage::ImageRgba8(image::RgbaImage::new(10, 10));
    let res =
        detect_color_frequency_distribution(&uniform_img).expect("uniform image should analyze");
    assert!(res.abs() < f64::EPSILON);
}

#[test]
fn sample_unique_color_count_is_exact_for_single_color_image() {
    let img = image::DynamicImage::new_rgba8(50, 50);
    let count = sample_unique_color_count(&img, 100).expect("blank image should sample");
    assert_eq!(count, 1);
}

#[test]
fn detect_format_from_bytes_recognizes_standard_magic_bytes() {
    let test_cases = [
        (
            vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A],
            DetectedFormat::PNG,
        ),
        (vec![0xFF, 0xD8, 0xFF, 0xE0], DetectedFormat::JPEG),
        (b"GIF89a".to_vec(), DetectedFormat::GIF),
    ];

    for (magic, expected) in test_cases {
        let mut temp = NamedTempFile::new().unwrap();
        temp.write_all(&magic).unwrap();
        if magic.len() < 32 {
            temp.write_all(&vec![0u8; 32 - magic.len()]).unwrap();
        }

        let result = detect_format_from_bytes(temp.path()).unwrap();
        assert_eq!(result, expected);
    }
}

#[test]
fn known_static_formats_short_circuit_animation_probe() {
    assert!(is_definitely_static_non_animated_format(
        &DetectedFormat::JPEG
    ));
    assert!(is_definitely_static_non_animated_format(
        &DetectedFormat::BMP
    ));
    assert!(!is_definitely_static_non_animated_format(
        &DetectedFormat::AVIF
    ));
    assert!(!is_definitely_static_non_animated_format(
        &DetectedFormat::PNG
    ));
}

#[test]
fn static_avif_brand_is_not_misclassified_as_animation() {
    let mut bytes = [0u8; 32];
    bytes[..4].copy_from_slice(&16u32.to_be_bytes());
    bytes[4..8].copy_from_slice(b"ftyp");
    bytes[8..12].copy_from_slice(b"avif");

    let mut temp = NamedTempFile::new().expect("temp avif");
    temp.write_all(&bytes).expect("write avif header");

    assert_eq!(
        detect_animation(temp.path(), &DetectedFormat::AVIF).expect("detect static AVIF"),
        (false, Some(1), None)
    );
}

#[test]
fn known_video_containers_short_circuit_as_animated() {
    assert!(is_definitely_animated_container(&DetectedFormat::MP4));
    assert!(is_definitely_animated_container(&DetectedFormat::WEBM));
    assert!(!is_definitely_animated_container(&DetectedFormat::JXL));
}

#[test]
fn parse_jxlinfo_animation_hint_detects_static_images() {
    let output = "\
JPEG XL image, 2x2, lossy, 8-bit RGB
Color space: RGB, D65, sRGB primaries, sRGB transfer function, rendering intent: Relative
";

    assert_eq!(parse_jxlinfo_animation_hint(output), Some(false));
}

#[test]
fn parse_jxlinfo_animation_hint_treats_zero_length_decoder_errors_as_static() {
    let output = "\
Decoder error
Animation length: 0.000 seconds
Error reading file: /tmp/broken.jxl
";

    assert_eq!(parse_jxlinfo_animation_hint(output), Some(false));
}

#[test]
fn measured_bit_depth_uses_jpeg_sof_sample_precision() {
    let mut temp = NamedTempFile::new().expect("temp jpeg");
    let jpeg = [
        0xFF, 0xD8, // SOI
        0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00, // APP0
        0xFF, 0xC0, // SOF0
        0x00, 0x11, // segment length
        0x0C, // 12-bit sample precision
        0x00, 0x10, // height
        0x00, 0x10, // width
        0x03, // components
        0x01, 0x11, 0x00, 0x02, 0x11, 0x01, 0x03, 0x11, 0x01, 0xFF, 0xD9, // EOI
    ];
    temp.write_all(&jpeg).expect("write jpeg");

    let precision = crate::conversion::jpeg_precision_from_header(temp.path());
    assert_eq!(precision.expect("jpeg precision probe"), Some(12));
    assert_eq!(
        measured_bit_depth_for_format(temp.path(), &DetectedFormat::JPEG),
        Some(12)
    );
}

fn default_factor_scores() -> PngQuantizationFactors {
    PngQuantizationFactors {
        dithering_detected: 0.0,
        color_count_anomaly: 0.0,
        gradient_banding: 0.0,
        color_frequency_distribution: 0.0,
        indexed_with_alpha: 0.0,
        large_palette: 0.0,
        tool_signature: 0.0,
        size_efficiency_anomaly: 0.0,
        entropy_anomaly: 0.0,
    }
}

#[test]
fn png_quality_estimator_respects_formula_constraints() {
    let factors = default_factor_scores();
    let min_q =
        crate::numeric_cast::f64_to_u8_strict(crate::constants::PNG_QUALITY_EST_MIN, "min_q")
            .unwrap();
    let max_q =
        crate::numeric_cast::f64_to_u8_strict(crate::constants::PNG_QUALITY_EST_MAX, "max_q")
            .unwrap();

    // 1. Range constraint
    let q = estimate_png_quantized_quality(None, Some(5.0), &factors, Some(1.0))
        .expect("PNG quality estimator should produce bounded u8");
    assert!(q >= min_q && q <= max_q);

    // 2. Monotonicity checks
    let q_large = estimate_png_quantized_quality(Some(256), Some(6.0), &factors, Some(1.0))
        .expect("large palette estimate should fit u8");
    let q_small = estimate_png_quantized_quality(Some(4), Some(6.0), &factors, Some(1.0))
        .expect("small palette estimate should fit u8");
    assert!(q_large > q_small);

    let q_high = estimate_png_quantized_quality(None, Some(7.5), &factors, Some(1.0))
        .expect("high entropy estimate should fit u8");
    let q_low = estimate_png_quantized_quality(None, Some(1.5), &factors, Some(1.0))
        .expect("low entropy estimate should fit u8");
    assert!(q_high >= q_low);

    // 3. Penalty factors
    let mut heavy = default_factor_scores();
    heavy.dithering_detected = 1.0;
    heavy.color_count_anomaly = 1.0;
    heavy.gradient_banding = 1.0;

    let q_heavy = estimate_png_quantized_quality(None, Some(5.0), &heavy, Some(1.0))
        .expect("heavy factor estimate should fit u8");
    let q_clean = estimate_png_quantized_quality(None, Some(5.0), &factors, Some(1.0))
        .expect("clean factor estimate should fit u8");
    assert!(q_heavy <= q_clean);
}

#[test]
fn apng_timing_stats_from_fctl_delays() {
    let data = super::synthetic_two_frame_apng_for_test();
    let stats = super::apng_timing_stats_from_bytes(&data).expect("APNG timing");
    assert_eq!(stats.frame_count, 2);
    assert!((stats.duration_secs - 0.03).abs() < 1.0e-6);
    assert!((stats.fps - (2.0 / 0.03)).abs() < 1.0e-3);
}

#[test]
fn detect_animation_png_apng_fps_from_fctl_delays_in_tempdir() {
    use std::io::Write;

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("anim.png");
    let data = super::synthetic_two_frame_apng_for_test();
    let mut file = std::fs::File::create(&path).expect("create png");
    file.write_all(&data).expect("write png");

    let (is_animated, frame_count, fps) =
        detect_animation(&path, &DetectedFormat::PNG).expect("detect_animation");
    assert!(is_animated);
    assert_eq!(frame_count, Some(2));
    let fps = fps.expect("fps from fcTL delays");
    assert!((f64::from(fps) - (2.0 / 0.03)).abs() < 1.0e-3);
}

#[test]
fn detect_animation_webp_fps_from_anmf_delays_in_tempdir() {
    use std::io::Write;

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("anim.webp");
    let data = crate::image_formats::webp::synthetic_two_frame_animated_webp_for_test();
    let mut file = std::fs::File::create(&path).expect("create webp");
    file.write_all(&data).expect("write webp");

    let (is_animated, frame_count, fps) =
        detect_animation(&path, &DetectedFormat::WebP).expect("detect_animation");
    assert!(is_animated);
    assert_eq!(frame_count, Some(2));
    let fps = fps.expect("fps from ANMF delays");
    assert!((f64::from(fps) - (2.0 / 0.3)).abs() < 1.0e-3);
}

#[test]
fn test_all_control_groups_lossless_lossy() {
    let formats = [
        ("WebP", "/tmp/test_lossless.webp", "/tmp/test_lossy.webp", DetectedFormat::WebP),
        ("AVIF", "/tmp/test_lossless.avif", "/tmp/test_lossy.avif", DetectedFormat::AVIF),
        ("TIFF", "/tmp/test_lossless.tiff", "/tmp/test_lossy.tiff", DetectedFormat::TIFF),
        ("JXL", "/tmp/test_lossless.jxl", "/tmp/test_lossy.jxl", DetectedFormat::JXL),
    ];
    
    for (name, lossless_path, lossy_path, format) in &formats {
        let l_path = std::path::Path::new(lossless_path);
        let y_path = std::path::Path::new(lossy_path);
        
        if l_path.exists() && y_path.exists() {
            let l_res = detect_compression(format, l_path);
            let y_res = detect_compression(format, y_path);
            
            println!("Control Group {name}: lossless={l_res:?}, lossy={y_res:?}");
            assert_eq!(l_res.unwrap(), CompressionType::Lossless, "Format {name} lossless was detected as lossy");
            assert_eq!(y_res.unwrap(), CompressionType::Lossy, "Format {name} lossy was detected as lossless");
        }
    }
}
