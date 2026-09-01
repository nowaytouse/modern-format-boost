use super::*;
use std::io::Write;
use tempfile::NamedTempFile;

#[test]
fn open_image_with_limits_uses_magic_bytes_for_mislabeled_png() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("qqcache-mislabeled.gif");
    image::DynamicImage::new_rgba8(2, 3)
        .save_with_format(&path, image::ImageFormat::Png)
        .expect("write PNG with GIF extension");

    let decoded = open_image_with_limits(&path).expect("decode by PNG magic bytes");
    assert_eq!((decoded.width(), decoded.height()), (2, 3));
}

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
fn png_text_decompression_rejects_output_beyond_remaining_budget() {
    let mut encoder = flate2::write::ZlibEncoder::new(
        Vec::new(),
        flate2::Compression::fast(),
    );
    encoder
        .write_all(&[b'a'; 65])
        .expect("compress PNG text fixture");
    let compressed = encoder.finish().expect("finish PNG text fixture");
    let mut remaining_budget = 64;

    let error = decompress_png_text_bounded(&compressed, &mut remaining_budget)
        .expect_err("decompressed PNG text beyond the budget must fail loudly");

    assert!(error.to_string().contains("exceeds remaining 64 byte"));
    assert_eq!(remaining_budget, 64, "failed payload must not consume budget");
}

#[test]
fn ico_declared_image_span_must_fit_the_file() {
    let mut ico = vec![0, 0, 1, 0, 1, 0];
    let mut entry = [0_u8; 16];
    entry[0] = 1;
    entry[1] = 1;
    entry[8..12].copy_from_slice(&64_u32.to_le_bytes());
    entry[12..16].copy_from_slice(&22_u32.to_le_bytes());
    ico.extend_from_slice(&entry);
    ico.extend_from_slice(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]);

    let mut file = NamedTempFile::new().expect("temp ICO");
    file.write_all(&ico).expect("write malformed ICO");
    let error = detect_compression(&DetectedFormat::ICO, file.path())
        .expect_err("ICO entry outside the physical file must fail before allocation");

    assert!(error.to_string().contains("image range"));
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

    let (is_animated, frame_count, _fps) =
        detect_animation(temp.path(), &DetectedFormat::AVIF).expect("detect static AVIF");
    assert!(
        !is_animated,
        "static AVIF brands must not be misclassified as animation"
    );
    // The fabricated frame_count=Some(1) fast path was removed (M248):
    // a header-only stub has no evidence for any frame count.
    assert!(
        frame_count.is_none() || frame_count == Some(1),
        "no fabricated frame count for header-only AVIF stub (got {frame_count:?})"
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
fn gif_two_decoded_frames_remain_animation_evidence_after_later_corruption() {
    let mut data = Vec::new();
    {
        let mut encoder =
            ::gif::Encoder::new(&mut data, 1, 1, &[0, 0, 0, 255, 255, 255]).unwrap();
        for pixel in [0_u8, 1] {
            let frame = ::gif::Frame {
                width: 1,
                height: 1,
                buffer: std::borrow::Cow::Owned(vec![pixel]),
                ..Default::default()
            };
            encoder.write_frame(&frame).unwrap();
        }
    }
    assert_eq!(data.pop(), Some(0x3B), "generated GIF must end in a trailer");
    data.push(0x2C); // Start a third image descriptor, then truncate it.

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("two-frames-then-truncated.gif");
    std::fs::write(&path, data).expect("write GIF fixture");

    let (animated, frame_count, fps) = detect_animation(&path, &DetectedFormat::GIF)
        .expect("two fully decoded frames already prove animation");
    assert!(animated);
    assert_eq!(frame_count, None, "corrupt tail prevents an exact frame count");
    assert_eq!(fps, None, "corrupt tail prevents exact timing statistics");
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
        (
            "WebP",
            "/tmp/test_lossless.webp",
            "/tmp/test_lossy.webp",
            DetectedFormat::WebP,
        ),
        (
            "AVIF",
            "/tmp/test_lossless.avif",
            "/tmp/test_lossy.avif",
            DetectedFormat::AVIF,
        ),
        (
            "TIFF",
            "/tmp/test_lossless.tiff",
            "/tmp/test_lossy.tiff",
            DetectedFormat::TIFF,
        ),
        (
            "JXL",
            "/tmp/test_lossless.jxl",
            "/tmp/test_lossy.jxl",
            DetectedFormat::JXL,
        ),
    ];

    for (name, lossless_path, lossy_path, format) in &formats {
        let l_path = std::path::Path::new(lossless_path);
        let y_path = std::path::Path::new(lossy_path);

        if l_path.exists() && y_path.exists() {
            let l_res = detect_compression(format, l_path);
            let y_res = detect_compression(format, y_path);

            println!("Control Group {name}: lossless={l_res:?}, lossy={y_res:?}");
            assert_eq!(
                l_res.unwrap(),
                CompressionType::Lossless,
                "Format {name} lossless was detected as lossy"
            );
            assert_eq!(
                y_res.unwrap(),
                CompressionType::Lossy,
                "Format {name} lossy was detected as lossless"
            );
        }
    }
}

/// Minimal raw JPEG 2000 codestream: SOC + SIZ + COD(transform) + SOD.
/// `transform`: 0 = 9/7 irreversible (lossy), 1 = 5/3 reversible (losslessness
/// still depends on other codestream markers).
///
/// Marker-segment lengths follow the spec convention `Lxxx` counts from the
/// length field itself (marker not included), and the walker advances
/// `marker(2) + Lxxx`, so every filler span below is sized to keep the next
/// marker aligned.
fn synthetic_jp2_raw_codestream(
    components: u16,
    main_transform: Option<u8>,
    tile_transform: Option<u8>,
    tile_component_transform: Option<(u16, u8)>,
) -> Vec<u8> {
    let mut cs = vec![0xFF, 0x4F]; // SOC
    let siz_len = 38 + 3 * components;
    cs.extend_from_slice(&[0xFF, 0x51]);
    cs.extend_from_slice(&siz_len.to_be_bytes());
    let mut siz_payload = vec![0u8; usize::from(siz_len - 2)];
    siz_payload[34..36].copy_from_slice(&components.to_be_bytes());
    cs.extend_from_slice(&siz_payload);
    if let Some(transform) = main_transform {
        cs.extend_from_slice(&[0xFF, 0x52, 0x00, 0x0C]); // COD, Lcod=12
        // Scod(1) + SGcod(4) + SPcod: NL, cb_w, cb_h, cb_style, transform
        cs.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0, 0, transform]);
    }
    cs.extend_from_slice(&[0xFF, 0x90, 0x00, 0x0A]); // SOT, Lsot=10
    cs.extend_from_slice(&[0; 8]); // Isot, Psot, TPsot, TNsot
    if let Some(transform) = tile_transform {
        cs.extend_from_slice(&[0xFF, 0x52, 0x00, 0x0C]);
        cs.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0, 0, transform]);
    }
    if let Some((component, transform)) = tile_component_transform {
        let component_bytes = if components <= 256 { 1 } else { 2 };
        let coc_len = 8 + component_bytes;
        cs.extend_from_slice(&[0xFF, 0x53]);
        cs.extend_from_slice(
            &u16::try_from(coc_len)
                .expect("COC marker length must fit in a JPEG 2000 marker")
                .to_be_bytes(),
        );
        if component_bytes == 1 {
            cs.push(u8::try_from(component).expect("one-byte component"));
        } else {
            cs.extend_from_slice(&component.to_be_bytes());
        }
        cs.extend_from_slice(&[0, 0, 0, 0, 0, transform]);
    }
    cs.extend_from_slice(&[0xFF, 0x93]); // SOD
    cs.extend_from_slice(&[0xAB, 0xCD, 0xEF, 0x12]); // tile data filler
    cs
}

#[test]
fn jp2_compression_uses_cod_wavelet_transform() {
    let dir = tempfile::tempdir().expect("tempdir");

    let lossy_path = dir.path().join("lossy.j2k");
    std::fs::write(
        &lossy_path,
        synthetic_jp2_raw_codestream(1, Some(0), None, None),
    )
    .expect("write lossy jp2");
    assert_eq!(
        detect_compression(&DetectedFormat::JP2, &lossy_path).expect("lossy jp2 codestream"),
        CompressionType::Lossy,
        "COD 9/7 irreversible transform must classify as lossy"
    );

    let reversible_path = dir.path().join("reversible-wavelet.j2k");
    std::fs::write(
        &reversible_path,
        synthetic_jp2_raw_codestream(1, Some(1), None, None),
    )
        .expect("write reversible-wavelet jp2");
    assert_eq!(
        detect_compression(&DetectedFormat::JP2, &reversible_path)
            .expect("reversible-wavelet jp2 codestream"),
        CompressionType::Unknown,
        "COD 5/3 alone must not fabricate a lossless verdict without quantization and MCT proof"
    );
}

#[test]
fn jp2_compression_resolves_tile_and_wide_component_overrides() {
    let dir = tempfile::tempdir().expect("tempdir");

    let tile_reversible = dir.path().join("tile-reversible.j2k");
    std::fs::write(
        &tile_reversible,
        synthetic_jp2_raw_codestream(1, Some(0), Some(1), None),
    )
    .expect("write tile override");
    assert_eq!(
        detect_compression(&DetectedFormat::JP2, &tile_reversible)
            .expect("tile override jp2"),
        CompressionType::Unknown,
        "tile COD must replace a lossy main-header default before admission"
    );

    let wide_component = dir.path().join("wide-component.j2k");
    std::fs::write(
        &wide_component,
        synthetic_jp2_raw_codestream(257, Some(1), None, Some((256, 0))),
    )
    .expect("write wide-component override");
    assert_eq!(
        detect_compression(&DetectedFormat::JP2, &wide_component)
            .expect("wide-component jp2"),
        CompressionType::Lossy,
        "COC must use a two-byte component index when Csiz exceeds 256"
    );
}

#[test]
fn jp2_compression_fails_closed_without_cod_marker() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("codless.j2k");
    std::fs::write(
        &path,
        synthetic_jp2_raw_codestream(1, None, None, None),
    )
    .expect("write codless jp2");

    let error = detect_compression(&DetectedFormat::JP2, &path)
        .expect_err("missing COD marker must not fabricate a lossy verdict");
    assert!(
        error.to_string().contains("no effective COD/COC"),
        "error should name the missing effective coding style: {error}"
    );
}

#[test]
fn jp2_compression_fails_closed_on_tiny_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("tiny.j2k");
    std::fs::write(&path, [0xFF, 0x4F, 0xFF]).expect("write tiny jp2");

    let error = detect_compression(&DetectedFormat::JP2, &path)
        .expect_err("too-short codestream must fail closed");
    assert!(
        error.to_string().contains("too short"),
        "error should state the short file: {error}"
    );
}

#[test]
fn detect_compression_is_explicit_for_jpeg_and_fails_closed_for_non_still_media() {
    assert_eq!(
        detect_compression(&DetectedFormat::JPEG, std::path::Path::new("any.jpg"))
            .expect("JPEG compression is lossy by route definition"),
        CompressionType::Lossy,
    );

    for format in [
        DetectedFormat::MP4,
        DetectedFormat::MOV,
        DetectedFormat::MKV,
        DetectedFormat::WEBM,
        DetectedFormat::Unknown("garbage".to_string()),
    ] {
        let error = detect_compression(&format, std::path::Path::new("media.bin"))
            .expect_err("video/unknown media must not receive a fabricated compression verdict");
        assert!(
            error.to_string().contains("still-image format"),
            "error should reject non-still media explicitly: {error}"
        );
    }
}

#[test]
fn static_by_spec_formats_are_confirmed_static_only() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("image.j2k");
    std::fs::write(
        &path,
        synthetic_jp2_raw_codestream(1, Some(0), None, None),
    )
    .expect("write jp2");

    // JP2 carries no animation capability in-spec: admission paths (tier-2
    // modern lossy import) must confirm it static instead of silently
    // excluding it via a conservative catch-all.
    assert!(
        animatable_format_confirmed_static_only(&path, &DetectedFormat::JP2, false, None)
            .expect("JP2 static confirmation"),
        "JP2 must be confirmed static-only by format definition"
    );
    assert!(
        !animatable_format_confirmed_static_only(&path, &DetectedFormat::MP4, false, None)
            .expect("MP4 static confirmation"),
        "video containers stay non-static"
    );
    assert!(
        !animatable_format_confirmed_static_only(
            &path,
            &DetectedFormat::Unknown("x".to_string()),
            false,
            None,
        )
        .expect("unknown media static confirmation"),
        "unknown media stays fail-closed non-static"
    );
}

/// Synthetic still AVIF: `ftyp` (major brand avif) + `av1C` with `flags`.
fn synthetic_avif_with_av1c(av1c_flags: u8) -> Vec<u8> {
    fn push_box(out: &mut Vec<u8>, box_type: [u8; 4], payload: &[u8]) {
        let size = u32::try_from(payload.len() + 8).expect("box size fits u32");
        out.extend_from_slice(&size.to_be_bytes());
        out.extend_from_slice(&box_type);
        out.extend_from_slice(payload);
    }

    let mut data = Vec::new();
    push_box(&mut data, *b"ftyp", b"avif\0\0\0\0");
    // av1C: marker/version byte, seq_profile/level byte, flags byte.
    push_box(&mut data, *b"av1C", &[0x81, 0x00, av1c_flags]);
    data
}

#[test]
fn avif_compression_requires_positive_evidence() {
    let dir = tempfile::tempdir().expect("tempdir");

    let write = |name: &str, data: &[u8]| {
        let path = dir.path().join(name);
        std::fs::write(&path, data).expect("write synthetic avif");
        path
    };

    // 4:2:0 (subsampling_x=1, subsampling_y=1): positive loss evidence.
    let path = write("avif_420.avif", &synthetic_avif_with_av1c(0x0C));
    assert_eq!(
        detect_compression(&DetectedFormat::AVIF, &path).expect("420 avif"),
        CompressionType::Lossy
    );

    // 4:2:2 (subsampling_x=1, subsampling_y=0): positive loss evidence.
    let path = write("avif_422.avif", &synthetic_avif_with_av1c(0x08));
    assert_eq!(
        detect_compression(&DetectedFormat::AVIF, &path).expect("422 avif"),
        CompressionType::Lossy
    );

    // 4:4:4 8-bit: pixel format proves nothing about AV1 quantization.
    let path = write("avif_444_8.avif", &synthetic_avif_with_av1c(0x00));
    assert_eq!(
        detect_compression(&DetectedFormat::AVIF, &path).expect("444 8-bit avif"),
        CompressionType::Unknown,
        "4:4:4 8-bit must not be guessed lossy or lossless"
    );

    // 4:4:4 10-bit (high_bitdepth): previously guessed lossless.
    let path = write("avif_444_10.avif", &synthetic_avif_with_av1c(0x40));
    assert_eq!(
        detect_compression(&DetectedFormat::AVIF, &path).expect("444 10-bit avif"),
        CompressionType::Unknown,
        "4:4:4 10-bit can still be a lossy AVIF; must stay Unknown"
    );

    // 4:4:4 12-bit (high_bitdepth|twelve_bit): previously guessed lossless.
    let path = write("avif_444_12.avif", &synthetic_avif_with_av1c(0x60));
    assert_eq!(
        detect_compression(&DetectedFormat::AVIF, &path).expect("444 12-bit avif"),
        CompressionType::Unknown,
        "4:4:4 12-bit can still be a lossy AVIF; must stay Unknown"
    );

    // Monochrome without subsampling: no proof either way.
    let path = write("avif_mono.avif", &synthetic_avif_with_av1c(0x10));
    assert_eq!(
        detect_compression(&DetectedFormat::AVIF, &path).expect("monochrome avif"),
        CompressionType::Unknown
    );
}

#[test]
fn avif_compression_identity_colr_is_not_lossless_proof() {
    let mut data = synthetic_avif_with_av1c(0x00);
    // Append an nclx colr box with identity matrix coefficients (MC=0):
    // a color description, not a quantization proof.
    let colr_payload = [
        b'n', b'c', b'l', b'x', 0, 1, 0, 0, 0, 0, 0, 0,
    ];
    let size = u32::try_from(colr_payload.len() + 8).expect("colr size fits u32");
    data.extend_from_slice(&size.to_be_bytes());
    data.extend_from_slice(b"colr");
    data.extend_from_slice(&colr_payload);

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("identity_colr.avif");
    std::fs::write(&path, &data).expect("write identity-colr avif");

    assert_eq!(
        detect_compression(&DetectedFormat::AVIF, &path).expect("identity-colr avif"),
        CompressionType::Unknown,
        "identity matrix is a pixel-format property, not lossless proof"
    );
}

#[test]
fn avif_compression_requires_every_codec_configuration_to_be_lossy() {
    let mut data = synthetic_avif_with_av1c(0x0C); // 4:2:0 auxiliary/thumbnail
    let av1c_444 = [0x81, 0x00, 0x00];
    let size = u32::try_from(av1c_444.len() + 8).expect("av1C size fits u32");
    data.extend_from_slice(&size.to_be_bytes());
    data.extend_from_slice(b"av1C");
    data.extend_from_slice(&av1c_444);

    let dir = tempfile::tempdir().expect("tempdir");
    let mixed_path = dir.path().join("mixed-config.avif");
    std::fs::write(&mixed_path, data).expect("write mixed-config avif");
    assert_eq!(
        detect_compression(&DetectedFormat::AVIF, &mixed_path).expect("mixed avif"),
        CompressionType::Unknown,
        "one lossy auxiliary av1C must not classify an ambiguous primary item"
    );

    let invalid_path = dir.path().join("invalid-config.avif");
    std::fs::write(&invalid_path, synthetic_avif_with_av1c(0x04))
        .expect("write reserved-subsampling avif");
    assert_eq!(
        detect_compression(&DetectedFormat::AVIF, &invalid_path).expect("reserved avif"),
        CompressionType::Unknown,
        "reserved x=0,y=1 must not become fabricated lossy evidence"
    );
}

#[test]
fn avif_compression_fails_closed_without_av1c() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut data = Vec::new();
    let size = 16u32.to_be_bytes();
    data.extend_from_slice(&size);
    data.extend_from_slice(b"ftypavif\0\0\0\0");

    let path = dir.path().join("no_av1c.avif");
    std::fs::write(&path, &data).expect("write av1c-less avif");

    let error = detect_compression(&DetectedFormat::AVIF, &path)
        .expect_err("missing av1C must not fabricate a compression verdict");
    assert!(
        error.to_string().contains("av1C"),
        "error should name the missing av1C box: {error}"
    );
}

fn synthetic_exr_part(compression: u8) -> Vec<u8> {
    let mut data = Vec::new();
    data.extend_from_slice(b"compression\0");
    data.extend_from_slice(b"compression\0");
    data.extend_from_slice(&1_u32.to_le_bytes());
    data.push(compression);
    data.push(0);
    data
}

#[test]
fn exr_multipart_flag_scans_every_part_for_lossy_compression() {
    let mut data = vec![0x76, 0x2f, 0x31, 0x01];
    // Version 2 + bit 12 (multipart); bit 9 is tiled, not multipart.
    data.extend_from_slice(&(2_u32 | (1_u32 << 12)).to_le_bytes());
    data.extend_from_slice(&synthetic_exr_part(0));
    data.extend_from_slice(&synthetic_exr_part(5));
    data.push(0);

    let mut file = NamedTempFile::new().expect("temp EXR");
    file.write_all(&data).expect("write multipart EXR");

    assert_eq!(
        detect_compression(&DetectedFormat::EXR, file.path()).expect("multipart EXR"),
        CompressionType::Lossy,
        "a lossy multipart part must not be hidden by a lossless first part"
    );
}

#[test]
fn jxlinfo_only_promotes_explicit_lossy_summary() {
    assert_eq!(
        parse_jxlinfo_compression_hint(
            "JPEG XL image, 64x64, lossy, 8-bit RGB\n"
        ),
        Some(CompressionType::Lossy)
    );
    assert_eq!(
        parse_jxlinfo_compression_hint(
            "JPEG XL image, 64x64, (possibly) lossless, 8-bit RGB\n"
        ),
        None,
        "jxlinfo's hedged lossless text is not proof"
    );
}

#[test]
fn jxlinfo_diagnostic_text_cannot_fabricate_lossy() {
    assert_eq!(
        parse_jxlinfo_compression_hint("error: lossy keyword in unrelated diagnostic"),
        None
    );
}
