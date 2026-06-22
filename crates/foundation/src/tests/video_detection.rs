// SANITY CHECKS: Consolidated pass-through testing for basic video format helper mapping.
use super::*;
use crate::media_precision::MediaPrecision;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

fn run_detected_codec_from_ffprobe() {
    assert_eq!(DetectedCodec::from_ffprobe("h264"), DetectedCodec::H264);
    assert_eq!(DetectedCodec::from_ffprobe("hevc"), DetectedCodec::H265);
    assert_eq!(DetectedCodec::from_ffprobe("vp9"), DetectedCodec::VP9);
    assert_eq!(DetectedCodec::from_ffprobe("av1"), DetectedCodec::AV1);
    assert_eq!(DetectedCodec::from_ffprobe("ffv1"), DetectedCodec::FFV1);
    assert!(matches!(
        DetectedCodec::from_ffprobe("unknown"),
        DetectedCodec::Unknown(_)
    ));
}

fn run_detected_codec_is_lossless() {
    assert!(DetectedCodec::FFV1.is_lossless());
    assert!(DetectedCodec::Uncompressed.is_lossless());
    assert!(!DetectedCodec::H264.is_lossless());
    assert!(!DetectedCodec::AV1.is_lossless());
}

fn run_color_space_parse() {
    assert_eq!(ColorSpace::parse("bt709"), ColorSpace::BT709);
    assert_eq!(ColorSpace::parse("bt2020"), ColorSpace::BT2020);
    assert_eq!(ColorSpace::parse("sRGB"), ColorSpace::SRGB);
    assert!(matches!(
        ColorSpace::parse("mystic"),
        ColorSpace::Unknown(_)
    ));
}

fn run_color_space_builder_mappings() {
    assert_eq!(
        ColorSpace::BT2020.yuv_output_colorspace(),
        Some(crate::constants::CS_BT2020)
    );
    assert_eq!(
        ColorSpace::BT709.yuv_output_colorspace(),
        Some(crate::constants::CS_BT709)
    );
    assert_eq!(ColorSpace::SRGB.yuv_output_colorspace(), None);
    assert_eq!(ColorSpace::AdobeRGB.yuv_output_colorspace(), None);
    assert_eq!(
        ColorSpace::Unknown("rgb".to_string()).yuv_output_colorspace(),
        Some("rgb")
    );
    assert_eq!(
        ColorSpace::Unknown(crate::constants::STR_UNKNOWN.to_string()).yuv_output_colorspace(),
        None
    );

    assert_eq!(
        ColorSpace::BT2020.quality_matcher_color_profile(),
        Some((crate::constants::CS_BT2020, true))
    );
    assert_eq!(
        ColorSpace::AdobeRGB.quality_matcher_color_profile(),
        Some((crate::constants::CS_ADOBE_RGB, false))
    );
    assert_eq!(
        ColorSpace::Unknown("mystic".to_string()).quality_matcher_color_profile(),
        None
    );
}

fn run_detection_is_hdr() {
    let mut det = Detection::default();
    assert!(!det.is_hdr());

    det.flags.hdr.is_dolby_vision = true;
    assert!(det.is_hdr());

    let det2 = Detection {
        color_transfer: Some("smpte2084".to_string()),
        ..Default::default()
    };
    assert!(det2.is_hdr());
}

fn run_detection_color_assessment_uses_shared_hdr_signal_priority() {
    let det = Detection {
        pix_fmt: "gbrpf32le".to_string(),
        color_space: ColorSpace::BT2020,
        color_transfer: Some(crate::constants::HDR_TRANSFER_PQ.to_string()),
        color_primaries: Some("bt2020".to_string()),
        flags: VideoFlags {
            hdr: VideoHdrFlags {
                is_dolby_vision: false,
                is_hdr10_plus: true,
            },
            ..Default::default()
        },
        ..Default::default()
    };

    let assessment = det.color_assessment();
    assert_eq!(
        assessment.hdr_signal(),
        Some(crate::ffprobe_json::HdrSignalKind::Hdr10Plus)
    );
    assert!(assessment.is_float());
    assert!(assessment.has_wide_gamut_signal());
}

fn run_detection_bit_depth_provenance() {
    let explicit = Detection {
        bit_depth: Some(10),
        ..Default::default()
    };
    assert_eq!(explicit.effective_bit_depth(), Some(10));
    assert_eq!(explicit.confirmed_bit_depth(), Some(10));
    assert!(explicit.should_preserve_high_bit_depth());
    assert!(explicit.has_confirmed_high_bit_depth());

    let inferred = Detection {
        bit_depth: Some(10),
        precision: VideoPrecisionMetadata {
            bit_depth_inferred_from_pix_fmt: true,
            ..Default::default()
        },
        compression: CompressionType::Lossless,
        ..Default::default()
    };
    assert_eq!(inferred.effective_bit_depth(), Some(10));
    assert_eq!(inferred.confirmed_bit_depth(), None);
    assert!(inferred.should_preserve_high_bit_depth());
    assert!(!inferred.has_confirmed_high_bit_depth());
    assert!(!inferred.is_high_fidelity());
}

fn run_determine_compression_type() {
    let precision = VideoPrecisionMetadata::default();
    let comp = determine_compression_type(
        &DetectedCodec::H264,
        Some(100_000_000), // Very high bitrate
        1920,
        1080,
        Some(60.0),
        &precision,
    );
    assert_eq!(comp, CompressionType::VisuallyLossless);

    let precision_lossless = VideoPrecisionMetadata {
        is_lossless_deterministic: true,
        ..Default::default()
    };
    let comp_lossless = determine_compression_type(
        &DetectedCodec::H264,
        None,
        1920,
        1080,
        None,
        &precision_lossless,
    );
    assert_eq!(comp_lossless, CompressionType::Lossless);
}

fn run_calculate_quality_score() {
    let score = calculate_quality_score(
        &CompressionType::Lossless,
        Some(10),
        false,
        None,
        3840,
        2160,
    );
    assert_eq!(score, 100);

    let inferred_score = calculate_quality_score(
        &CompressionType::HighQuality,
        Some(10),
        true,
        None,
        1920,
        1080,
    );
    let explicit_score = calculate_quality_score(
        &CompressionType::HighQuality,
        Some(10),
        false,
        None,
        1920,
        1080,
    );
    assert!(explicit_score > inferred_score);

    let score_low =
        calculate_quality_score(&CompressionType::LowQuality, Some(8), false, None, 640, 480);
    assert!(score_low < 50);
}

fn run_extract_video_precision() {
    let mut tags = HashMap::new();
    tags.insert("comment".to_string(), "crf=18.5 preset=slower".to_string());

    let precision = extract_video_precision(&tags, None, Some(4));
    assert_eq!(precision.original_crf, Some(18.5));
    assert_eq!(precision.original_preset, Some("slower".to_string()));
    assert_eq!(precision.original_max_b_frames, Some(4));
}

fn run_generate_video_recommendation() {
    let features = Detection {
        codec: DetectedCodec::ProRes,
        file_path: "test.mov".to_string(),
        ..Default::default()
    };

    let rec = generate_video_recommendation(&features);
    assert!(rec.is_archival_upgrade);
    assert!(rec.recommended_codec.contains("AV1"));
}

#[derive(Debug, Clone, Copy)]
enum RealHevcColorFixtureKind {
    Hdr10Plus,
    WideGamutSdr,
}

struct RealHevcColorFixture {
    _temp_dir: TempDir,
    path: PathBuf,
}

impl RealHevcColorFixture {
    fn hdr10plus_x265_params(metadata_path: &std::path::Path) -> String {
        let mut params = format!(
            "colorprim=bt2020:transfer={}:colormatrix=bt2020nc",
            crate::constants::HDR_TRANSFER_PQ
        );
        crate::append_x265_hdr10_params(
            &mut params,
            Some("bt2020nc"),
            Some(crate::constants::HDR_TRANSFER_PQ),
            Some("bt2020"),
            Some("G(8500,39850)B(6550,2300)R(35400,14600)WP(15635,16450)L(10000000,1)"),
            Some("1000,400"),
            true,
            "yuv420p10le",
            Some(metadata_path),
        );
        params
    }

    fn environment_ready() -> bool {
        if which::which(crate::constants::TOOL_FFMPEG).is_err()
            || which::which(crate::constants::TOOL_FFPROBE).is_err()
        {
            eprintln!("skipping real HEVC color detection test: ffmpeg/ffprobe unavailable");
            return false;
        }

        let encoder_help = Command::new(crate::constants::TOOL_FFMPEG)
            .args(["-hide_banner", "-h", "encoder=libx265"])
            .output();

        match encoder_help {
            Ok(output) if output.status.success() => true,
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                eprintln!(
                    "skipping real HEVC color detection test: libx265 unavailable ({})",
                    stderr.trim()
                );
                false
            }
            Err(err) => {
                eprintln!(
                    "skipping real HEVC color detection test: cannot inspect libx265 support ({err})"
                );
                false
            }
        }
    }

    fn generate(kind: RealHevcColorFixtureKind) -> Result<Self, String> {
        let temp_dir = tempfile::tempdir().map_err(|err| err.to_string())?;
        // MKV keeps x265 HDR10+ dynamic metadata in-band; MP4 mux often drops frame side_data.
        let output_path = temp_dir.path().join(match kind {
            RealHevcColorFixtureKind::Hdr10Plus => "hdr10plus_sample.mkv",
            RealHevcColorFixtureKind::WideGamutSdr => "bt2020_sdr_sample.mkv",
        });

        let x265_params = match kind {
            RealHevcColorFixtureKind::Hdr10Plus => {
                let metadata_path = temp_dir.path().join("hdr10plus.json");
                fs::write(&metadata_path, Self::hdr10_plus_metadata_json())
                    .map_err(|err| format!("failed to write HDR10+ metadata JSON: {err}"))?;
                Self::hdr10plus_x265_params(&metadata_path)
            }
            RealHevcColorFixtureKind::WideGamutSdr => {
                "repeat-headers=1:colorprim=bt2020:transfer=bt709:colormatrix=bt2020nc".to_string()
            }
        };

        let output = Command::new(crate::constants::TOOL_FFMPEG)
            .args([
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=128x72:rate=24",
                "-frames:v",
                "24",
                "-pix_fmt",
                "yuv420p10le",
                "-c:v",
                crate::constants::LIB_X265,
                "-preset",
                "ultrafast",
                "-x265-params",
            ])
            .arg(&x265_params)
            .arg(&output_path)
            .output()
            .map_err(|err| format!("failed to launch ffmpeg for fixture generation: {err}"))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!(
                "ffmpeg fixture generation failed for {:?}: {}",
                kind,
                stderr.trim()
            ));
        }

        if matches!(kind, RealHevcColorFixtureKind::Hdr10Plus)
            && !Self::probe_has_hdr10_plus_side_data(&output_path)
        {
            Self::materialize_committed_hdr10plus_fixture(&output_path)?;
            if !Self::probe_has_hdr10_plus_side_data(&output_path) {
                return Err(
                    "committed HDR10+ fixture is missing typed frame side_data".to_string(),
                );
            }
        }

        Ok(Self {
            _temp_dir: temp_dir,
            path: output_path,
        })
    }

    fn probe_has_hdr10_plus_side_data(path: &std::path::Path) -> bool {
        match crate::ffprobe::probe_video(path) {
            Ok(probe) => probe.hdr.hdr10_plus,
            Err(err) => {
                eprintln!("HDR10+ side-data probe failed for {}: {err}", path.display());
                false
            }
        }
    }

    fn committed_hdr10plus_fixture_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/real_hevc_hdr10plus.mkv")
    }

    fn materialize_committed_hdr10plus_fixture(output_path: &std::path::Path) -> Result<(), String> {
        let fixture = Self::committed_hdr10plus_fixture_path();
        if !fixture.is_file() {
            return Err(format!(
                "HDR10+ sample missing typed side_data and committed fixture not found at {}",
                fixture.display()
            ));
        }
        fs::copy(&fixture, output_path)
            .map(|_| ())
            .map_err(|err| format!("failed to copy committed HDR10+ fixture: {err}"))
    }

    fn hdr10_plus_metadata_json() -> &'static str {
        r#"[{"BezierCurveData":{"Anchor0":461,"Anchor1":584,"Anchor10":971,"Anchor11":989,"Anchor12":1002,"Anchor13":1011,"Anchor2":651,"Anchor3":708,"Anchor4":761,"Anchor5":808,"Anchor6":851,"Anchor7":888,"Anchor8":921,"Anchor9":949,"KneePointX":0,"KneePointY":0,"NumberOfAnchors":14},"LocalParameters":[],"LuminanceParameters":{"AverageRGB":13925,"MaxScl0":39521,"MaxScl1":39521,"MaxScl2":39521,"PercentileLuminance":{"NumberOfPercentiles":10,"PercentileLuminance0":0,"PercentileLuminance1":1,"PercentileLuminance2":2,"PercentileLuminance3":79,"PercentileLuminance4":2514,"PercentileLuminance5":39521,"PercentileLuminance6":39522,"PercentileLuminance7":39523,"PercentileLuminance8":39524,"PercentileLuminance9":39525,"PercentilePercentage0":1,"PercentilePercentage1":5,"PercentilePercentage2":10,"PercentilePercentage3":25,"PercentilePercentage4":50,"PercentilePercentage5":75,"PercentilePercentage6":90,"PercentilePercentage7":95,"PercentilePercentage8":98,"PercentilePercentage9":99}},"NumberOfWindows":1,"SceneFrameIndex":0,"SceneId":0,"TargetedSystemDisplayMaximumLuminance":400}]"#
    }
}

#[test]
fn test_video_detection_sanity_pass_through() {
    run_detected_codec_from_ffprobe();
    run_detected_codec_is_lossless();
    run_color_space_parse();
    run_color_space_builder_mappings();
    run_detection_is_hdr();
    run_detection_color_assessment_uses_shared_hdr_signal_priority();
    run_detection_bit_depth_provenance();
    run_determine_compression_type();
    run_calculate_quality_score();
    run_extract_video_precision();
    run_generate_video_recommendation();
}

#[test]
fn test_real_hevc_color_signal_chain_distinguishes_hdr10_plus_from_bt2020_sdr() {
    crate::test_ci_contract::require_ffmpeg_toolchain_in_ci("HEVC HDR10+ color signal chain");
    crate::test_ci_contract::require_libx265_encoder_in_ci("HEVC HDR10+ color signal chain");
    if !RealHevcColorFixture::environment_ready() {
        return;
    }

    let hdr10_plus = RealHevcColorFixture::generate(RealHevcColorFixtureKind::Hdr10Plus)
        .unwrap_or_else(|err| panic!("failed to generate real HDR10+ sample: {err}"));
    let hdr_probe = probe_video(&hdr10_plus.path)
        .unwrap_or_else(|err| panic!("ffprobe failed on generated HDR10+ sample: {err}"));
    assert_eq!(
        hdr_probe.color_transfer.as_deref(),
        Some(crate::constants::HDR_TRANSFER_PQ)
    );
    assert_eq!(hdr_probe.color_primaries.as_deref(), Some("bt2020"));
    assert_eq!(hdr_probe.color_space.as_deref(), Some("bt2020nc"));
    assert!(hdr_probe.hdr.hdr10_plus);
    assert!(hdr_probe.hdr.has_explicit_hdr_metadata());
    assert!(hdr_probe.is_hdr());
    assert_eq!(
        hdr_probe.color_assessment().hdr_signal(),
        Some(crate::ffprobe_json::HdrSignalKind::Hdr10Plus)
    );

    let hdr_detection = detect_video(&hdr10_plus.path)
        .unwrap_or_else(|err| panic!("detect_video failed on generated HDR10+ sample: {err}"));
    assert_eq!(hdr_detection.color_space, ColorSpace::BT2020);
    assert_eq!(
        hdr_detection.color_transfer.as_deref(),
        Some(crate::constants::HDR_TRANSFER_PQ)
    );
    assert_eq!(hdr_detection.color_primaries.as_deref(), Some("bt2020"));
    assert!(hdr_detection.flags.hdr.is_hdr10_plus);
    assert!(hdr_detection.is_hdr());

    let bt2020_sdr = RealHevcColorFixture::generate(RealHevcColorFixtureKind::WideGamutSdr)
        .unwrap_or_else(|err| panic!("failed to generate real BT.2020 SDR sample: {err}"));
    let sdr_probe = probe_video(&bt2020_sdr.path)
        .unwrap_or_else(|err| panic!("ffprobe failed on generated BT.2020 SDR sample: {err}"));
    assert_eq!(sdr_probe.color_transfer.as_deref(), Some("bt709"));
    assert_eq!(sdr_probe.color_primaries.as_deref(), Some("bt2020"));
    assert_eq!(sdr_probe.color_space.as_deref(), Some("bt2020nc"));
    assert!(!sdr_probe.hdr.hdr10_plus);
    assert!(!sdr_probe.hdr.has_explicit_hdr_metadata());
    assert!(!sdr_probe.is_hdr());
    assert!(sdr_probe.color_assessment().has_wide_gamut_signal());

    let sdr_detection = detect_video(&bt2020_sdr.path)
        .unwrap_or_else(|err| panic!("detect_video failed on generated BT.2020 SDR sample: {err}"));
    assert_eq!(sdr_detection.color_space, ColorSpace::BT2020);
    assert_eq!(sdr_detection.color_transfer.as_deref(), Some("bt709"));
    assert_eq!(sdr_detection.color_primaries.as_deref(), Some("bt2020"));
    assert!(!sdr_detection.flags.hdr.is_hdr10_plus);
    assert!(!sdr_detection.is_hdr());
    assert!(sdr_detection.color_assessment().has_wide_gamut_signal());
}

#[test]
#[serial_test::serial]
fn video_quality_db_fusion_skips_when_gate_disabled() {
    use super::{CompressionType, Detection, apply_video_quality_db_fusion};
    use crate::common_utils::EnvGuard;
    use std::path::Path;

    let _guard = EnvGuard::set(
        crate::constants::ENV_DISABLE_SCENARIO_QUALITY_DB_FUSION,
        "1",
    );
    let mut detection = Detection {
        compression: CompressionType::Standard,
        quality_score: 77,
        ..Default::default()
    };
    apply_video_quality_db_fusion(&mut detection, Path::new("/tmp/nonexistent.mp4"));
    assert_eq!(detection.quality_score, 77);
}

#[test]
fn video_quality_db_fusion_skips_lossless_compression() {
    use super::{CompressionType, Detection, apply_video_quality_db_fusion};
    use std::path::Path;

    for compression in [CompressionType::Lossless, CompressionType::VisuallyLossless] {
        let mut detection = Detection {
            compression,
            quality_score: 88,
            ..Default::default()
        };
        apply_video_quality_db_fusion(&mut detection, Path::new("/tmp/nonexistent.mp4"));
        assert_eq!(detection.quality_score, 88,
            "lossless tiers must not invoke scenario DB fusion"
        );
    }
}

fn minimal_animated_webp_fixture() -> Vec<u8> {
  // RIFF animated WebP: VP8X (100×80) + ANIM — ffprobe may leave frame_count empty.
    vec![
        b'R', b'I', b'F', b'F', 0x1E, 0, 0, 0, b'W', b'E', b'B', b'P', b'V', b'P', b'8', b'X', 10,
        0, 0, 0, 0x02, 0, 0, 0, 99, 0, 0, 79, 0, 0, b'A', b'N', b'I', b'M', 0, 0, 0, 0,
    ]
}

fn minimal_animated_gif_fixture() -> Vec<u8> {
    let mut gif_data = Vec::new();
    {
        let mut encoder = ::gif::Encoder::new(&mut gif_data, 10, 8, &[0, 0, 0, 255, 255, 255])
            .expect("gif encoder");
        let buf0 = [0u8];
        let buf1 = [1u8];
        encoder
            .write_frame(&::gif::Frame {
                delay: 10,
                width: 1,
                height: 1,
                buffer: std::borrow::Cow::Borrowed(&buf0),
                ..Default::default()
            })
            .expect("gif frame 1");
        encoder
            .write_frame(&::gif::Frame {
                delay: 20,
                width: 1,
                height: 1,
                buffer: std::borrow::Cow::Borrowed(&buf1),
                ..Default::default()
            })
            .expect("gif frame 2");
    }
    gif_data
}

#[test]
fn cached_detection_bitstream_repair_m128() {
    use super::{
        cached_detection_needs_bitstream_repair, Detection,
        repair_animated_container_detection_from_bitstream_header,
    };
    use std::io::Write;
    use tempfile::TempDir;

    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("anim.webp");
    let mut file = std::fs::File::create(&path).expect("create webp");
    file.write_all(&minimal_animated_webp_fixture())
        .expect("write webp");

    let stale = Detection {
        width: None,
        height: None,
        frame_count: Some(1),
        ..Default::default()
    };
    assert!(cached_detection_needs_bitstream_repair(&stale, &path));

    let mut repaired = stale;
    repair_animated_container_detection_from_bitstream_header(&path, &mut repaired)
        .expect("repair stale webp detection");
    assert!(!cached_detection_needs_bitstream_repair(&repaired, &path));
}

#[test]
fn repair_detection_canvas_and_frames_m127() {
    use super::{Detection, repair_animated_container_detection_from_bitstream_header};
    use std::io::Write;
    use tempfile::TempDir;

    let dir = TempDir::new().expect("tempdir");

    let webp_path = dir.path().join("anim.webp");
    let mut file = std::fs::File::create(&webp_path).expect("create webp");
    file.write_all(&minimal_animated_webp_fixture())
        .expect("write webp");
    let mut webp_det = Detection {
        width: None,
        height: None,
        frame_count: Some(1),
        ..Default::default()
    };
    repair_animated_container_detection_from_bitstream_header(&webp_path, &mut webp_det)
        .expect("repair webp detection");
    assert_eq!(webp_det.width, Some(100));
    assert_eq!(webp_det.height, Some(80));
    assert_eq!(webp_det.frame_count, Some(2));

    let gif_path = dir.path().join("anim.gif");
    let mut file = std::fs::File::create(&gif_path).expect("create gif");
    file.write_all(&minimal_animated_gif_fixture())
        .expect("write gif");
    let mut gif_det = Detection {
        width: Some(0),
        height: Some(0),
        frame_count: None,
        ..Default::default()
    };
    repair_animated_container_detection_from_bitstream_header(&gif_path, &mut gif_det)
        .expect("repair gif detection");
    assert_eq!(gif_det.width, Some(10));
    assert_eq!(gif_det.height, Some(8));
    assert_eq!(gif_det.frame_count, Some(2));
}

#[test]
fn detect_video_animated_apng_header_preflight_m125() {
    use super::detect_video;
    use std::io::Write;
    use tempfile::TempDir;

    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("anim.png");
    let data = crate::image_detection::synthetic_two_frame_apng_for_test();
    let mut file = std::fs::File::create(&path).expect("create png");
    file.write_all(&data).expect("write png");

    let detection = detect_video(&path).expect("detect animated apng via header preflight");
    assert_eq!(detection.width, Some(1));
    assert_eq!(detection.height, Some(1));
    assert_eq!(detection.frame_count, Some(2));
}

#[test]
fn detect_video_animated_gif_header_preflight_m124() {
    use super::detect_video;
    use std::io::Write;
    use tempfile::TempDir;

    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("anim.gif");
    let mut file = std::fs::File::create(&path).expect("create gif");
    file.write_all(&minimal_animated_gif_fixture())
        .expect("write gif");

    let detection = detect_video(&path).expect("detect animated gif via header preflight");
    assert_eq!(detection.width, Some(10));
    assert_eq!(detection.height, Some(8));
    assert_eq!(detection.frame_count, Some(2));
}

#[test]
fn detect_video_animated_webp_header_preflight_m123() {
    use super::detect_video;
    use std::io::Write;
    use tempfile::TempDir;

    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("anim.webp");
    let mut file = std::fs::File::create(&path).expect("create webp");
    file.write_all(&minimal_animated_webp_fixture())
        .expect("write webp");

    let detection = detect_video(&path).expect("detect animated webp via header preflight");
    assert_eq!(detection.width, Some(100));
    assert_eq!(detection.height, Some(80));
    assert_eq!(detection.frame_count, Some(2));
}

#[test]
fn promote_animated_container_for_vid_webp_header_recovery() {
    use super::{Detection, promote_animated_container_for_vid};
    use std::io::Write;
    use tempfile::TempDir;

    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("anim.webp");
    let mut file = std::fs::File::create(&path).expect("create webp");
    file.write_all(&minimal_animated_webp_fixture())
        .expect("write webp");

    let mut detection = Detection {
        frame_count: Some(0),
        width: None,
        height: None,
        ..Default::default()
    };
    assert!(promote_animated_container_for_vid(&path, &mut detection).expect("promote webp"));
    assert_eq!(detection.frame_count, Some(2));
    assert_eq!(detection.width, Some(100));
    assert_eq!(detection.height, Some(80));
}

#[test]
fn promote_animated_container_skips_when_frame_count_already_multi() {
    use super::{Detection, promote_animated_container_for_vid};
    use std::io::Write;
    use tempfile::TempDir;

    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("anim.webp");
    let mut file = std::fs::File::create(&path).expect("create webp");
    file.write_all(&minimal_animated_webp_fixture())
        .expect("write webp");

    let mut detection = Detection {
        frame_count: Some(24),
        ..Default::default()
    };
    assert!(!promote_animated_container_for_vid(&path, &mut detection).expect("promote webp"));
    assert_eq!(detection.frame_count, Some(24));
}
