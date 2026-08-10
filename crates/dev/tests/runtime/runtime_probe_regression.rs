use std::io::Write;
use std::path::Path;

use foundation::image_detection::{
    DetectedFormat, animatable_format_confirmed_static_only, detect_animation,
    detect_format_from_bytes, is_isobmff_animated_sequence,
};

include!("../edge/apng/synth_animated_apng.rs");
include!("../edge/avif/synth_static_avif.rs");
include!("../edge/webp/synth_animated_webp.rs");
include!("../edge/heic/synth_static_heic.rs");
include!("../edge/jxl/synth_static_jxl.rs");

#[test]
fn runtime_probe_regression_suite() {
    animated_webp_header_preflight_detect_video();
    animated_apng_header_preflight_detect_video();
    static_heic_format_and_animation_probe();
    static_heif_mif1_compat_format_probe();
    static_avif_format_and_animation_probe();
    static_jxl_short_header_format_probe();
    static_jxl_long_header_format_probe();
    static_jxl_detect_video_single_frame();
    static_heic_detect_video_honest_probe();
    animated_avif_avis_sequence_probe();
    animated_heif_msf1_sequence_probe();
    isobmff_sequence_brand_matrix_probe();
    avis_gate_static_only_rejection_probe();
    msf1_gate_static_only_rejection_probe();
    static_heic_cover_stream_not_ambiguous_probe();
    fabricated_multi_frame_never_confirmed_static_probe();
    static_heic_minimal_moov_still_static_probe();
    static_mif1_compat_not_sequence_probe();
    zero_tolerance_quality_embed_nan_slots_probe();
    real_dual_track_isobmff_probe();
    animated_jxl_probe();
}

fn animated_webp_header_preflight_detect_video() {
    let bytes = build_synthetic_two_frame_animated_webp();
    let mut temp = tempfile::NamedTempFile::with_suffix(".webp").expect("temp webp");
    temp.write_all(&bytes).expect("write synthetic webp");
    let path = temp.path();

    let detection = foundation::video_detection::detect_video(path)
        .unwrap_or_else(|e| panic!("detect_video must succeed on synthetic animated WebP: {e:?}"));

    assert_eq!(detection.width, Some(100), "WebP preflight width");
    assert_eq!(detection.height, Some(80), "WebP preflight height");
    assert_eq!(
        detection.frame_count,
        Some(2),
        "WebP header preflight must not fabricate frame count"
    );
}

fn animated_apng_header_preflight_detect_video() {
    let bytes = build_synthetic_two_frame_apng();
    let mut temp = tempfile::NamedTempFile::with_suffix(".png").expect("temp apng");
    temp.write_all(&bytes).expect("write synthetic apng");
    let path = temp.path();

    let detection = foundation::video_detection::detect_video(path)
        .unwrap_or_else(|e| panic!("detect_video must succeed on synthetic APNG: {e:?}"));

    assert_eq!(detection.width, Some(1), "APNG preflight width");
    assert_eq!(detection.height, Some(1), "APNG preflight height");
    assert_eq!(
        detection.frame_count,
        Some(2),
        "APNG acTL preflight must report measured frame count"
    );
    assert!(
        Path::new(path).extension().is_some_and(|ext| ext == "png"),
        "APNG regression uses .png container"
    );
}

fn static_heic_format_and_animation_probe() {
    let bytes = build_synthetic_static_heic_ftyp();
    let mut temp = tempfile::NamedTempFile::with_suffix(".heic").expect("temp heic");
    temp.write_all(&bytes).expect("write synthetic heic");
    let path = temp.path();

    let fmt = detect_format_from_bytes(path).unwrap_or_else(|e| {
        panic!("detect_format_from_bytes must succeed on synthetic HEIC: {e:?}")
    });
    assert!(
        matches!(fmt, DetectedFormat::HEIC | DetectedFormat::HEIF),
        "HEIC ftyp major brand must resolve to DetectedFormat::HEIC or HEIF (got {fmt:?})"
    );

    let (is_animated, frame_count, _fps) = detect_animation(path, &fmt)
        .unwrap_or_else(|e| panic!("detect_animation must succeed on synthetic HEIC: {e:?}"));
    assert!(
        !is_animated,
        "header-only HEIC stub must not be treated as animated"
    );
    assert!(
        frame_count.is_none() || frame_count == Some(1),
        "static HEIC must not fabricate frame count (got {frame_count:?})"
    );
}

fn static_heif_mif1_compat_format_probe() {
    let bytes = build_synthetic_mif1_heic_compat_ftyp();
    let mut temp = tempfile::NamedTempFile::with_suffix(".heif").expect("temp heif");
    temp.write_all(&bytes).expect("write synthetic heif");
    let path = temp.path();

    let fmt = detect_format_from_bytes(path).unwrap_or_else(|e| {
        panic!("detect_format_from_bytes must succeed on mif1/heic stub: {e:?}")
    });
    assert!(
        matches!(fmt, DetectedFormat::HEIC),
        "mif1 + heic compatible brand must disambiguate to HEIC"
    );
}

fn static_avif_format_and_animation_probe() {
    let bytes = build_synthetic_static_avif_ftyp();
    let mut temp = tempfile::NamedTempFile::with_suffix(".avif").expect("temp avif");
    temp.write_all(&bytes).expect("write synthetic avif");
    let path = temp.path();

    let fmt = detect_format_from_bytes(path).unwrap_or_else(|e| {
        panic!("detect_format_from_bytes must succeed on synthetic AVIF: {e:?}")
    });
    assert!(
        matches!(fmt, DetectedFormat::AVIF),
        "AVIF ftyp major brand must resolve to DetectedFormat::AVIF"
    );

    let (is_animated, frame_count, _fps) = detect_animation(path, &fmt)
        .unwrap_or_else(|e| panic!("detect_animation must succeed on synthetic AVIF: {e:?}"));
    assert!(
        !is_animated,
        "header-only AVIF stub must not be treated as animated"
    );
    assert!(
        frame_count.is_none() || frame_count == Some(1),
        "static AVIF must not fabricate frame count (got {frame_count:?})"
    );
}

fn static_jxl_short_header_format_probe() {
    let bytes = build_synthetic_jxl_short_header();
    let mut temp = tempfile::NamedTempFile::with_suffix(".jxl").expect("temp jxl");
    temp.write_all(&bytes).expect("write synthetic jxl short");
    let path = temp.path();

    let fmt = detect_format_from_bytes(path)
        .unwrap_or_else(|e| panic!("detect_format_from_bytes must succeed on short JXL: {e:?}"));
    assert!(matches!(fmt, DetectedFormat::JXL));

    let (is_animated, frame_count, _fps) = detect_animation(path, &fmt)
        .unwrap_or_else(|e| panic!("detect_animation must succeed on short JXL: {e:?}"));
    assert!(!is_animated, "short-header JXL stub must be static");
    assert!(
        frame_count.is_none() || frame_count == Some(1),
        "static JXL must not fabricate frame count (got {frame_count:?})"
    );
}

fn static_jxl_long_header_format_probe() {
    let bytes = build_synthetic_jxl_long_header();
    let mut temp = tempfile::NamedTempFile::with_suffix(".jxl").expect("temp jxl");
    temp.write_all(&bytes).expect("write synthetic jxl long");
    let path = temp.path();

    let fmt = detect_format_from_bytes(path)
        .unwrap_or_else(|e| panic!("detect_format_from_bytes must succeed on long JXL: {e:?}"));
    assert!(matches!(fmt, DetectedFormat::JXL));

    let (is_animated, frame_count, _fps) = detect_animation(path, &fmt)
        .unwrap_or_else(|e| panic!("detect_animation must succeed on long JXL: {e:?}"));
    assert!(!is_animated, "long-header JXL stub must be static");
    assert!(
        frame_count.is_none() || frame_count == Some(1),
        "static JXL must not fabricate frame count (got {frame_count:?})"
    );
}

fn static_jxl_detect_video_single_frame() {
    let bytes = build_synthetic_jxl_short_header();
    let mut temp = tempfile::NamedTempFile::with_suffix(".jxl").expect("temp jxl");
    temp.write_all(&bytes).expect("write synthetic jxl");
    let path = temp.path();

    match foundation::video_detection::detect_video(path) {
        Ok(detection) => {
            assert_eq!(
                detection.frame_count,
                Some(1),
                "static JXL detect_video must force single-frame when probe succeeds"
            );
        }
        Err(_err) => {
            // Header-only JXL may fail ffprobe dimension parse; static path
            // covered by format probe.
        }
    }
}

fn static_heic_detect_video_honest_probe() {
    let bytes = build_synthetic_static_heic_ftyp();
    let mut temp = tempfile::NamedTempFile::with_suffix(".heic").expect("temp heic");
    temp.write_all(&bytes).expect("write synthetic heic");
    let path = temp.path();

    match foundation::video_detection::detect_video(path) {
        Ok(detection) => {
            assert!(
                detection.frame_count.is_none() || detection.frame_count == Some(1),
                "header-only HEIC must not fabricate multi-frame animation (got {:?})",
                detection.frame_count
            );
        }
        Err(_err) => {
            // Honest ffprobe failure on header-only stub is acceptable.
        }
    }
}

fn animated_avif_avis_sequence_probe() {
    let bytes = build_synthetic_animated_avif_avis_ftyp();
    let mut temp = tempfile::NamedTempFile::with_suffix(".avif").expect("temp avif avis");
    temp.write_all(&bytes).expect("write synthetic avis avif");
    let path = temp.path();

    let fmt = detect_format_from_bytes(path)
        .unwrap_or_else(|e| panic!("detect_format_from_bytes on avis AVIF: {e:?}"));
    assert!(matches!(fmt, DetectedFormat::AVIF));

    let (is_animated, frame_count, _fps) = detect_animation(path, &fmt)
        .unwrap_or_else(|e| panic!("detect_animation on avis AVIF: {e:?}"));
    assert!(
        is_animated,
        "avis major brand must be detected as animated ISOBMFF sequence"
    );
    assert!(
        frame_count != Some(1),
        "animated avis stub must not be downgraded to static frame_count=1 (got {frame_count:?})"
    );
}

fn animated_heif_msf1_sequence_probe() {
    let bytes = build_synthetic_animated_heif_msf1_ftyp();
    let mut temp = tempfile::NamedTempFile::with_suffix(".heif").expect("temp heif msf1");
    temp.write_all(&bytes).expect("write synthetic msf1 heif");
    let path = temp.path();

    let fmt = detect_format_from_bytes(path)
        .unwrap_or_else(|e| panic!("detect_format_from_bytes on msf1 HEIF: {e:?}"));
    assert!(
        matches!(fmt, DetectedFormat::HEIC | DetectedFormat::HEIF),
        "msf1/heic ftyp must resolve to HEIC or HEIF (got {fmt:?})"
    );

    let (is_animated, frame_count, _fps) = detect_animation(path, &fmt)
        .unwrap_or_else(|e| panic!("detect_animation on msf1 HEIF: {e:?}"));
    assert!(
        is_animated,
        "msf1 major brand must be detected as animated ISOBMFF sequence"
    );
    assert!(
        frame_count != Some(1),
        "animated msf1 stub must not be downgraded to static frame_count=1 (got {frame_count:?})"
    );
}

fn isobmff_sequence_brand_matrix_probe() {
    let static_heic = write_temp_isobmff(".heic", &build_synthetic_static_heic_ftyp());
    assert!(
        !is_isobmff_animated_sequence(static_heic.path())
            .unwrap_or_else(|e| panic!("is_isobmff_animated_sequence static HEIC: {e:?}")),
        "static heic major brand must not be a sequence brand"
    );

    let avis = write_temp_isobmff(".avif", &build_synthetic_animated_avif_avis_ftyp());
    assert!(
        is_isobmff_animated_sequence(avis.path())
            .unwrap_or_else(|e| panic!("is_isobmff_animated_sequence avis AVIF: {e:?}")),
        "avis major brand must be an animated sequence"
    );

    let msf1 = write_temp_isobmff(".heif", &build_synthetic_animated_heif_msf1_ftyp());
    assert!(
        is_isobmff_animated_sequence(msf1.path())
            .unwrap_or_else(|e| panic!("is_isobmff_animated_sequence msf1 HEIF: {e:?}")),
        "msf1 major brand must be an animated sequence"
    );
}

fn avis_gate_static_only_rejection_probe() {
    let path = write_temp_isobmff(".avif", &build_synthetic_animated_avif_avis_ftyp());
    let fmt = detect_format_from_bytes(path.path())
        .unwrap_or_else(|e| panic!("detect_format_from_bytes avis: {e:?}"));
    assert!(matches!(fmt, DetectedFormat::AVIF));
    let (is_animated, frame_count, _) = detect_animation(path.path(), &fmt)
        .unwrap_or_else(|e| panic!("detect_animation avis: {e:?}"));
    assert!(is_animated, "avis must be animated");
    let confirmed =
        animatable_format_confirmed_static_only(path.path(), &fmt, is_animated, frame_count)
            .unwrap_or_else(|e| panic!("animatable_format_confirmed_static_only avis: {e:?}"));
    assert!(
        !confirmed,
        "animated avis must not pass animatable_format_confirmed_static_only"
    );
}

fn msf1_gate_static_only_rejection_probe() {
    let path = write_temp_isobmff(".heif", &build_synthetic_animated_heif_msf1_ftyp());
    let fmt = detect_format_from_bytes(path.path())
        .unwrap_or_else(|e| panic!("detect_format_from_bytes msf1: {e:?}"));
    let (is_animated, frame_count, _) = detect_animation(path.path(), &fmt)
        .unwrap_or_else(|e| panic!("detect_animation msf1: {e:?}"));
    assert!(is_animated, "msf1 must be animated");
    let confirmed =
        animatable_format_confirmed_static_only(path.path(), &fmt, is_animated, frame_count)
            .unwrap_or_else(|e| panic!("animatable_format_confirmed_static_only msf1: {e:?}"));
    assert!(
        !confirmed,
        "animated msf1 must not pass animatable_format_confirmed_static_only"
    );
}

fn static_heic_cover_stream_not_ambiguous_probe() {
    let path = write_temp_isobmff(".heic", &build_synthetic_static_heic_ftyp());
    assert!(
        !foundation::ffprobe::isobmff_cover_stream_ambiguous(path.path()),
        "header-only static HEIC stub must not trip cover-stream ambiguity heuristics"
    );
}

fn fabricated_multi_frame_never_confirmed_static_probe() {
    let path = write_temp_isobmff(".heic", &build_synthetic_static_heic_ftyp());
    let fmt = DetectedFormat::HEIC;
    let confirmed = animatable_format_confirmed_static_only(path.path(), &fmt, true, Some(2))
        .unwrap_or_else(|e| panic!("animatable_format_confirmed_static_only fabricated: {e:?}"));
    assert!(
        !confirmed,
        "fabricated multi-frame animation must never be confirmed static-only"
    );
}

fn static_heic_minimal_moov_still_static_probe() {
    let path = write_temp_isobmff(".heic", &build_synthetic_static_heic_ftyp_moov());
    let fmt = detect_format_from_bytes(path.path())
        .unwrap_or_else(|e| panic!("detect_format_from_bytes heic+moov: {e:?}"));
    assert!(matches!(fmt, DetectedFormat::HEIC | DetectedFormat::HEIF));
    let (is_animated, frame_count, _) = detect_animation(path.path(), &fmt)
        .unwrap_or_else(|e| panic!("detect_animation heic+moov: {e:?}"));
    assert!(
        !is_animated,
        "minimal moov static HEIC must not be classified as animated (got fc={frame_count:?})"
    );
    assert!(
        frame_count != Some(2),
        "must not fabricate multi-frame count on moov-only stub (got {frame_count:?})"
    );
}

fn zero_tolerance_quality_embed_nan_slots_probe() {
    use foundation::image_analyzer::{ImageAnalysis, ImageFeatures};
    use foundation::image_quality_db::{
        QUALITY_EMBED_COLOR_DEPTH_SLOT, QUALITY_EMBED_MISSING_MEASUREMENT, QUALITY_EMBED_PSNR_SLOT,
        QUALITY_EMBED_SSIM_SLOT, get_quality_features,
    };

    let analysis = ImageAnalysis {
        width: 640,
        height: 480,
        file_size: 100_000,
        format: "PNG".to_string(),
        psnr: None,
        ssim: None,
        features: ImageFeatures {
            entropy: Some(7.0),
            compression_ratio: Some(1.2),
        },
        physics_225: Some(vec![0.5; 225]),
        ..ImageAnalysis::default()
    };

    let embedding = get_quality_features(&analysis).expect("quality embedding");
    let slice = embedding.as_slice();

    // CONTRACT (M225/M246): unmeasured optional slots carry the pgvector-safe
    // missing-measurement sentinel (-1.0), NOT NaN. pgvector rejects NaN on INSERT;
    // the normalization step in get_quality_features converts NaN → sentinel before
    // returning. The design contract is documented in:
    //   test_get_quality_features_uses_pgvector_safe_missing_measurement_sentinel
    // and in assert_quality_embedding_finite_policy which explicitly rejects NaN.
    let sentinel = QUALITY_EMBED_MISSING_MEASUREMENT;
    assert!(
        (slice[QUALITY_EMBED_PSNR_SLOT] - sentinel).abs() <= f32::EPSILON,
        "unmeasured PSNR embed slot must carry pgvector-safe sentinel {sentinel}, got {}",
        slice[QUALITY_EMBED_PSNR_SLOT]
    );
    assert!(
        (slice[QUALITY_EMBED_SSIM_SLOT] - sentinel).abs() <= f32::EPSILON,
        "unmeasured SSIM embed slot must carry pgvector-safe sentinel {sentinel}, got {}",
        slice[QUALITY_EMBED_SSIM_SLOT]
    );
    // Entire embedding must be finite — pgvector rejects non-finite values.
    assert!(
        slice.iter().all(|v| v.is_finite()),
        "full embedding must be finite for pgvector storage; non-finite slots: {:?}",
        slice
            .iter()
            .enumerate()
            .filter(|(_, v)| !v.is_finite())
            .collect::<Vec<_>>()
    );

    let no_depth = ImageAnalysis {
        color_depth: None,
        ..analysis
    };
    let no_depth_embed = get_quality_features(&no_depth).expect("embedding without bit depth");
    assert!(
        (no_depth_embed.as_slice()[QUALITY_EMBED_COLOR_DEPTH_SLOT] - sentinel).abs()
            <= f32::EPSILON,
        "unknown bit depth must carry pgvector-safe sentinel {sentinel} in slot \
         {QUALITY_EMBED_COLOR_DEPTH_SLOT}, not 0.0 or NaN"
    );
}

fn static_mif1_compat_not_sequence_probe() {
    let path = write_temp_isobmff(".heif", &build_synthetic_mif1_heic_compat_ftyp());
    assert!(
        !is_isobmff_animated_sequence(path.path())
            .unwrap_or_else(|e| panic!("is_isobmff_animated_sequence mif1: {e:?}")),
        "mif1/heic compat still image must not use sequence brands"
    );
    let fmt = detect_format_from_bytes(path.path())
        .unwrap_or_else(|e| panic!("detect_format_from_bytes mif1: {e:?}"));
    let (is_animated, frame_count, _) = detect_animation(path.path(), &fmt)
        .unwrap_or_else(|e| panic!("detect_animation mif1: {e:?}"));
    assert!(!is_animated, "mif1 compat stub must remain static");
    assert_ne!(
        frame_count,
        Some(2),
        "must not invent animation frame_count (got {frame_count:?})"
    );
}

fn write_temp_isobmff(suffix: &str, bytes: &[u8]) -> tempfile::NamedTempFile {
    let mut temp =
        tempfile::NamedTempFile::with_suffix(suffix).expect("temp isobmff regression file");
    temp.write_all(bytes)
        .expect("write synthetic isobmff bytes");
    temp
}

fn real_dual_track_isobmff_probe() {
    let path = Path::new("crates/dev/src/fixtures/dual_track.mp4");
    if !path.exists() {
        return; // Skip if not generated
    }
    let fmt = detect_format_from_bytes(path)
        .unwrap_or_else(|e| panic!("detect_format_from_bytes dual_track: {e:?}"));
    let (is_animated, frame_count, _) = detect_animation(path, &fmt)
        .unwrap_or_else(|e| panic!("detect_animation dual_track: {e:?}"));
    assert!(is_animated, "dual track ISOBMFF must be animated");
    assert!(
        frame_count.is_none() || frame_count.unwrap() > 1,
        "dual track must not be downgraded to 1 frame (got {frame_count:?})"
    );
}

fn animated_jxl_probe() {
    let path = Path::new("crates/dev/src/fixtures/animated.jxl");
    if !path.exists() {
        return; // Skip if not generated
    }
    let fmt = detect_format_from_bytes(path)
        .unwrap_or_else(|e| panic!("detect_format_from_bytes animated.jxl: {e:?}"));
    assert!(matches!(fmt, DetectedFormat::JXL));
    let (is_animated, frame_count, _) = detect_animation(path, &fmt)
        .unwrap_or_else(|e| panic!("detect_animation animated.jxl: {e:?}"));
    assert!(is_animated, "animated JXL must be animated");
    assert!(
        frame_count.is_none() || frame_count.unwrap() > 1,
        "animated JXL must not be downgraded to 1 frame (got {frame_count:?})"
    );
}
