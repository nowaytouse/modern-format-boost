// VIDEO QUALITY TESTS: Monotonicity mathematical proof + consolidated pass-through quality profile verification.
use super::*;

// Part 1: High-value mathematical property tests (kept as is)

#[cfg(test)]
mod property_tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn smoke_crf_monotonicity(bpp1 in 0.01_f64..10.0f64, bpp2 in 0.01_f64..10.0f64) {
            let crf1 = estimate_crf_from_bpp(bpp1, VideoCodecType::ModernEfficient);
            let crf2 = estimate_crf_from_bpp(bpp2, VideoCodecType::ModernEfficient);
            if bpp1 > bpp2 {
                prop_assert!(crf1 <= crf2);
            } else if bpp1 < bpp2 {
                prop_assert!(crf1 >= crf2);
            }
        }
    }
}

// Part 2: Consolidated pass-through sanity runners (trivial tests downgraded)

fn run_smoke_crf_estimation_boundaries() {
    assert_eq!(estimate_crf_from_bpp(5.0, VideoCodecType::Unknown), 18);
    assert_eq!(estimate_crf_from_bpp(5.0001, VideoCodecType::Unknown), 14);
    assert_eq!(estimate_crf_from_bpp(1.0, VideoCodecType::Unknown), 22);
    assert_eq!(estimate_crf_from_bpp(1.0001, VideoCodecType::Unknown), 18);
    assert_eq!(estimate_crf_from_bpp(0.08, VideoCodecType::Unknown), 35);
    assert_eq!(estimate_crf_from_bpp(0.0001, VideoCodecType::Unknown), 35);
}

fn run_smoke_codec_quality_profiling() {
    // 1. Legacy H.264 1080p
    let h264 = analyze_video_quality(VideoQualityInput {
        codec: "h264",
        width: Some(1920),
        height: Some(1080),
        fps: Some(30.0),
        duration_secs: Some(60.0),
        total_bitrate: Some(8_000_000),
        video_bitrate: Some(7_500_000),
        pix_fmt: "yuv420p",
        bit_depth: Some(8),
        bit_depth_inferred_from_pix_fmt: false,
        max_b_frames: Some(2),
        encoder_params: None,
        gop_size: Some(60),
        color_space: Some("bt709"),
        color_transfer: None,
        color_primaries: None,
        has_mastering_display: false,
        has_max_cll: false,
        is_dolby_vision: false,
        is_hdr10_plus: false,
        file_size: 60_000_000,
        frame_count: None,
    })
    .unwrap();
    assert_eq!(h264.codec_type, VideoCodecType::Legacy);
    assert!(!h264.flags.decision.should_skip);

    // 2. HEVC 4K HDR
    let hevc = analyze_video_quality(VideoQualityInput {
        codec: "hevc",
        width: Some(3840),
        height: Some(2160),
        fps: Some(30.0),
        duration_secs: Some(120.0),
        total_bitrate: Some(20_000_000),
        video_bitrate: Some(19_000_000),
        pix_fmt: "yuv420p10le",
        bit_depth: Some(10),
        bit_depth_inferred_from_pix_fmt: false,
        max_b_frames: Some(2),
        encoder_params: None,
        gop_size: Some(60),
        color_space: Some("bt2020nc"),
        color_transfer: Some(crate::constants::HDR_TRANSFER_PQ),
        color_primaries: Some("bt2020"),
        has_mastering_display: false,
        has_max_cll: false,
        is_dolby_vision: false,
        is_hdr10_plus: false,
        file_size: 300_000_000,
        frame_count: None,
    })
    .unwrap();
    assert_eq!(hevc.codec_type, VideoCodecType::ModernEfficient);
    assert!(hevc.flags.decision.should_skip);
    assert!(hevc.flags.features.is_hdr);

    // 3. ProRes Intermediate
    let prores = analyze_video_quality(VideoQualityInput {
        codec: "prores",
        width: Some(1920),
        height: Some(1080),
        fps: Some(24.0),
        duration_secs: Some(60.0),
        total_bitrate: Some(150_000_000),
        video_bitrate: Some(145_000_000),
        pix_fmt: "yuv422p10le",
        bit_depth: Some(10),
        bit_depth_inferred_from_pix_fmt: false,
        max_b_frames: Some(0),
        encoder_params: None,
        gop_size: Some(1),
        color_space: Some("bt709"),
        color_transfer: None,
        color_primaries: None,
        has_mastering_display: false,
        has_max_cll: false,
        is_dolby_vision: false,
        is_hdr10_plus: false,
        file_size: 1_125_000_000,
        frame_count: None,
    })
    .unwrap();
    assert_eq!(prores.codec_type, VideoCodecType::Intermediate);
    assert_eq!(prores.chroma, ChromaSubsampling::Yuv422);

    // 4. FFV1 Lossless
    let ffv1 = analyze_video_quality(VideoQualityInput {
        codec: "ffv1",
        width: Some(1920),
        height: Some(1080),
        fps: Some(30.0),
        duration_secs: Some(30.0),
        total_bitrate: Some(200_000_000),
        video_bitrate: Some(195_000_000),
        pix_fmt: "yuv444p",
        bit_depth: Some(8),
        bit_depth_inferred_from_pix_fmt: false,
        max_b_frames: Some(0),
        encoder_params: None,
        gop_size: Some(1),
        color_space: None,
        color_transfer: None,
        color_primaries: None,
        has_mastering_display: false,
        has_max_cll: false,
        is_dolby_vision: false,
        is_hdr10_plus: false,
        file_size: 750_000_000,
        frame_count: None,
    })
    .unwrap();
    assert_eq!(ffv1.codec_type, VideoCodecType::Lossless);
    assert_eq!(ffv1.chroma, ChromaSubsampling::Yuv444);
}

fn run_smoke_codec_skipping_rules() {
    for codec in ["hevc", "av1", "vp9", "vvc"] {
        let result = analyze_video_quality(VideoQualityInput {
            codec,
            width: Some(1920),
            height: Some(1080),
            fps: Some(30.0),
            duration_secs: Some(60.0),
            total_bitrate: Some(8_000_000),
            video_bitrate: None,
            pix_fmt: "yuv420p",
            bit_depth: Some(8),
            bit_depth_inferred_from_pix_fmt: false,
            max_b_frames: Some(2),
            encoder_params: None,
            gop_size: None,
            color_space: None,
            color_transfer: None,
            color_primaries: None,
            has_mastering_display: false,
            has_max_cll: false,
            is_dolby_vision: false,
            is_hdr10_plus: false,
            file_size: 60_000_000,
            frame_count: None,
        })
        .unwrap();
        assert!(result.flags.decision.should_skip);
    }

    for codec in ["h264", "mjpeg", "prores"] {
        let result = analyze_video_quality(VideoQualityInput {
            codec,
            width: Some(1920),
            height: Some(1080),
            fps: Some(30.0),
            duration_secs: Some(60.0),
            total_bitrate: Some(8_000_000),
            video_bitrate: None,
            pix_fmt: "yuv420p",
            bit_depth: Some(8),
            bit_depth_inferred_from_pix_fmt: false,
            max_b_frames: Some(2),
            encoder_params: None,
            gop_size: None,
            color_space: None,
            color_transfer: None,
            color_primaries: None,
            has_mastering_display: false,
            has_max_cll: false,
            is_dolby_vision: false,
            is_hdr10_plus: false,
            file_size: 60_000_000,
            frame_count: None,
        })
        .unwrap();
        assert!(!result.flags.decision.should_skip);
    }
}

fn run_smoke_chroma_parsing() {
    assert_eq!(
        ChromaSubsampling::from_pix_fmt("yuv420p"),
        ChromaSubsampling::Yuv420
    );
    assert_eq!(
        ChromaSubsampling::from_pix_fmt("yuv420p10le"),
        ChromaSubsampling::Yuv420
    );
    assert_eq!(
        ChromaSubsampling::from_pix_fmt("yuv422p"),
        ChromaSubsampling::Yuv422
    );
    assert_eq!(
        ChromaSubsampling::from_pix_fmt("yuv444p"),
        ChromaSubsampling::Yuv444
    );
    assert_eq!(
        ChromaSubsampling::from_pix_fmt("rgb24"),
        ChromaSubsampling::Rgb
    );

    assert!((ChromaSubsampling::Yuv420.quality_factor() - 1.0).abs() < 0.01_f64);
    assert!(ChromaSubsampling::Yuv422.quality_factor() > 1.0_f64);
    assert!(
        ChromaSubsampling::Yuv444.quality_factor() > ChromaSubsampling::Yuv422.quality_factor()
    );
}

fn run_smoke_invalid_inputs() {
    // 0 fps
    let res_fps = analyze_video_quality(VideoQualityInput {
        codec: "h264",
        width: Some(1920),
        height: Some(1080),
        fps: Some(0.0),
        duration_secs: Some(60.0),
        total_bitrate: Some(8_000_000),
        video_bitrate: None,
        pix_fmt: "yuv420p",
        bit_depth: Some(8),
        bit_depth_inferred_from_pix_fmt: false,
        max_b_frames: Some(2),
        encoder_params: None,
        gop_size: None,
        color_space: None,
        color_transfer: None,
        color_primaries: None,
        has_mastering_display: false,
        has_max_cll: false,
        is_dolby_vision: false,
        is_hdr10_plus: false,
        file_size: 60_000_000,
        frame_count: None,
    });
    assert!(res_fps.is_err());

    // 0 duration
    let res_dur = analyze_video_quality(VideoQualityInput {
        codec: "h264",
        width: Some(1920),
        height: Some(1080),
        fps: Some(30.0),
        duration_secs: Some(0.0),
        total_bitrate: Some(8_000_000),
        video_bitrate: None,
        pix_fmt: "yuv420p",
        bit_depth: Some(8),
        bit_depth_inferred_from_pix_fmt: false,
        max_b_frames: Some(2),
        encoder_params: None,
        gop_size: None,
        color_space: None,
        color_transfer: None,
        color_primaries: None,
        has_mastering_display: false,
        has_max_cll: false,
        is_dolby_vision: false,
        is_hdr10_plus: false,
        file_size: 60_000_000,
        frame_count: None,
    });
    assert!(res_dur.is_err());
}

fn run_smoke_params_and_metadata() {
    // bpp calculations
    let result = analyze_video_quality(VideoQualityInput {
        codec: "h264",
        width: Some(1920),
        height: Some(1080),
        fps: Some(30.0),
        duration_secs: Some(60.0),
        total_bitrate: Some(8_000_000),
        video_bitrate: Some(8_000_000),
        pix_fmt: "yuv420p",
        bit_depth: Some(8),
        bit_depth_inferred_from_pix_fmt: false,
        max_b_frames: Some(2),
        encoder_params: None,
        gop_size: None,
        color_space: None,
        color_transfer: None,
        color_primaries: None,
        has_mastering_display: false,
        has_max_cll: false,
        is_dolby_vision: false,
        is_hdr10_plus: false,
        file_size: 60_000_000,
        frame_count: None,
    })
    .unwrap();
    let expected_bpp = 8_000_000.0_f64 / (1_920.0_f64 * 1_080.0_f64 * 30.0_f64);
    assert!((result.bpp - expected_bpp).abs() < 0.001_f64);

    // crf parsing
    assert_eq!(extract_crf_from_params("crf=23.5").unwrap(), Some(24u8));
    assert_eq!(
        extract_crf_from_params("x265 [info]: CRF 18.0").unwrap(),
        Some(18u8)
    );
    assert!(
        extract_crf_from_params("rc=crf / crf=not-a-number / preset=medium").is_err(),
        "malformed explicit CRF metadata must not fall back to estimated CRF"
    );

    let result_crf = analyze_video_quality(VideoQualityInput {
        codec: "h264",
        width: Some(1920),
        height: Some(1080),
        fps: Some(30.0),
        duration_secs: Some(60.0),
        total_bitrate: Some(1_000_000),
        video_bitrate: None,
        pix_fmt: "yuv420p",
        bit_depth: Some(8),
        bit_depth_inferred_from_pix_fmt: false,
        max_b_frames: Some(2),
        encoder_params: Some("rc=crf / crf=15.0 / preset=medium"),
        gop_size: None,
        color_space: None,
        color_transfer: None,
        color_primaries: None,
        has_mastering_display: false,
        has_max_cll: false,
        is_dolby_vision: false,
        is_hdr10_plus: false,
        file_size: 7_500_000,
        frame_count: None,
    })
    .unwrap();
    assert_eq!(result_crf.estimated_crf, 15);

    let mut no_forgery = analyze_video_quality(VideoQualityInput {
        codec: "h264",
        width: Some(1920),
        height: Some(1080),
        fps: Some(30.0),
        duration_secs: Some(60.0),
        total_bitrate: Some(8_000_000),
        video_bitrate: Some(8_000_000),
        pix_fmt: "yuv420p",
        bit_depth: Some(8),
        bit_depth_inferred_from_pix_fmt: false,
        max_b_frames: Some(2),
        encoder_params: None,
        gop_size: None,
        color_space: None,
        color_transfer: None,
        color_primaries: None,
        has_mastering_display: false,
        has_max_cll: false,
        is_dolby_vision: false,
        is_hdr10_plus: false,
        file_size: 60_000_000,
        frame_count: Some(1800),
    })
    .unwrap();
    no_forgery.flags.features.is_hdr = true;

    let quality = to_quality_analysis(&no_forgery);
    assert_eq!(quality.gop_size, None);
    assert_eq!(quality.color_space, None);
    assert_eq!(quality.is_hdr, Some(true));
}

#[test]
fn test_video_quality_sanity_pass_through() {
    run_smoke_crf_estimation_boundaries();
    run_smoke_codec_quality_profiling();
    run_smoke_codec_skipping_rules();
    run_smoke_chroma_parsing();
    run_smoke_invalid_inputs();
    run_smoke_params_and_metadata();
}

#[test]
fn test_inferred_bit_depth_does_not_claim_quality_bonus() {
    let explicit_input = VideoQualityInput {
        codec: "hevc",
        width: Some(1920),
        height: Some(1080),
        fps: Some(30.0),
        duration_secs: Some(60.0),
        total_bitrate: Some(8_000_000),
        video_bitrate: Some(7_500_000),
        pix_fmt: "yuv420p10le",
        bit_depth: Some(10),
        bit_depth_inferred_from_pix_fmt: false,
        max_b_frames: Some(2),
        encoder_params: None,
        gop_size: Some(60),
        color_space: Some("bt709"),
        color_transfer: None,
        color_primaries: None,
        has_mastering_display: false,
        has_max_cll: false,
        is_dolby_vision: false,
        is_hdr10_plus: false,
        file_size: 60_000_000,
        frame_count: None,
    };
    let explicit = analyze_video_quality(explicit_input).unwrap();

    let inferred = analyze_video_quality(VideoQualityInput {
        bit_depth_inferred_from_pix_fmt: true,
        ..explicit_input
    })
    .unwrap();

    assert!(explicit.quality_score > inferred.quality_score);
}

#[test]
fn test_video_quality_hdr_detection_reuses_shared_assessment() {
    let sdr_bt2020 = analyze_video_quality(VideoQualityInput {
        codec: "hevc",
        width: Some(1920),
        height: Some(1080),
        fps: Some(30.0),
        duration_secs: Some(60.0),
        total_bitrate: Some(8_000_000),
        video_bitrate: Some(7_500_000),
        pix_fmt: "yuv420p10le",
        bit_depth: Some(10),
        bit_depth_inferred_from_pix_fmt: false,
        max_b_frames: Some(2),
        encoder_params: None,
        gop_size: Some(60),
        color_space: Some("bt2020nc"),
        color_transfer: None,
        color_primaries: Some("bt2020"),
        has_mastering_display: false,
        has_max_cll: false,
        is_dolby_vision: false,
        is_hdr10_plus: false,
        file_size: 60_000_000,
        frame_count: None,
    })
    .unwrap();
    assert!(!sdr_bt2020.flags.features.is_hdr);

    let hdr10_plus = analyze_video_quality(VideoQualityInput {
        is_hdr10_plus: true,
        ..sdr_bt2020_to_input()
    })
    .unwrap();
    assert!(hdr10_plus.flags.features.is_hdr);
}

fn sdr_bt2020_to_input() -> VideoQualityInput<'static> {
    VideoQualityInput {
        codec: "hevc",
        width: Some(1920),
        height: Some(1080),
        fps: Some(30.0),
        duration_secs: Some(60.0),
        total_bitrate: Some(8_000_000),
        video_bitrate: Some(7_500_000),
        pix_fmt: "yuv420p10le",
        bit_depth: Some(10),
        bit_depth_inferred_from_pix_fmt: false,
        max_b_frames: Some(2),
        encoder_params: None,
        gop_size: Some(60),
        color_space: Some("bt2020nc"),
        color_transfer: None,
        color_primaries: Some("bt2020"),
        has_mastering_display: false,
        has_max_cll: false,
        is_dolby_vision: false,
        is_hdr10_plus: false,
        file_size: 60_000_000,
        frame_count: None,
    }
}
