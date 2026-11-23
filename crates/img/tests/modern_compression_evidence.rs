//! End-to-end compression-evidence regression against real encoder output.
//!
//! Tool-gated (cjxl/avifenc): follows the repo convention of skipping with an
//! explicit stderr note when a tool is unavailable. When the tools exist, the
//! classifications below are asserted strictly — Unknown is only ever the
//! expected answer where codec evidence is genuinely unprovable.

use foundation::image_detection::{CompressionType, DetectedFormat, detect_compression};
use foundation::scan_modern_lossy_static_candidates;
use std::process::{Command, Stdio};
use tempfile::TempDir;

const ONE_BY_ONE_RGBA_PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE,
    0x42, 0x60, 0x82,
];

fn tool_available(tool: &str) -> bool {
    match Command::new(tool)
        .arg("--help")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
    {
        Ok(status) => status.success(),
        Err(_err) => false,
    }
}

fn write_input_png(dir: &TempDir) -> anyhow::Result<std::path::PathBuf> {
    let path = dir.path().join("input.png");
    std::fs::write(&path, ONE_BY_ONE_RGBA_PNG)?;
    Ok(path)
}

fn run_encoder(
    dir: &TempDir,
    tool: &str,
    args: &[&str],
    output_name: &str,
) -> anyhow::Result<std::path::PathBuf> {
    let output = dir.path().join(output_name);
    let status = Command::new(tool)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    anyhow::ensure!(
        status.success(),
        "{tool} failed with {status} while producing {output_name}"
    );
    anyhow::ensure!(output.is_file(), "{tool} did not produce {output_name}");
    Ok(output)
}

fn utf8(path: &std::path::Path) -> anyhow::Result<&str> {
    path.to_str()
        .ok_or_else(|| anyhow::anyhow!("non-UTF-8 temp path: {}", path.display()))
}

#[test]
fn jxl_compression_evidence_from_real_cjxl_output() -> anyhow::Result<()> {
    if !tool_available("cjxl") {
        eprintln!("Skipping jxl compression evidence test: cjxl is unavailable");
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let input = write_input_png(&dir)?;
    let input_arg = utf8(&input)?;

    // VarDCT lossy (default -d 1 path): positive lossy evidence.
    let lossy = run_encoder(
        &dir,
        "cjxl",
        &[
            input_arg,
            utf8(&dir.path().join("vardct.jxl"))?,
            "-d",
            "1",
            "-e",
            "3",
        ],
        "vardct.jxl",
    )?;
    assert_eq!(
        detect_compression(&DetectedFormat::JXL, &lossy)?,
        CompressionType::Lossy,
        "VarDCT JXL must classify as ConfirmedLossy"
    );

    // Modular d=0: even libjxl's own jxlinfo hedges this as '(possibly)
    // lossless' — container/codestream headers cannot prove modular lossless,
    // so the honest verdict is Unknown.
    let modular = run_encoder(
        &dir,
        "cjxl",
        &[
            input_arg,
            utf8(&dir.path().join("modular.jxl"))?,
            "-d",
            "0",
            "-e",
            "3",
        ],
        "modular.jxl",
    )?;
    assert_eq!(
        detect_compression(&DetectedFormat::JXL, &modular)?,
        CompressionType::Unknown,
        "modular lossless cannot be proven from codestream metadata; must stay Unknown"
    );

    // Modular mode is not synonymous with lossless. libjxl's own inspector
    // positively labels a non-zero-distance modular stream as lossy, so the
    // shared detector must not hide it behind Unknown.
    let modular_lossy = run_encoder(
        &dir,
        "cjxl",
        &[
            input_arg,
            utf8(&dir.path().join("modular-lossy.jxl"))?,
            "-m",
            "1",
            "-d",
            "2",
            "-e",
            "3",
        ],
        "modular-lossy.jxl",
    )?;
    assert_eq!(
        detect_compression(&DetectedFormat::JXL, &modular_lossy)?,
        CompressionType::Lossy,
        "positive libjxl evidence must admit modular-lossy JXL"
    );
    let tier2_scan =
        scan_modern_lossy_static_candidates(dir.path(), &[modular, modular_lossy.clone()])?;
    assert_eq!(tier2_scan.probe_failures, Vec::new());
    assert_eq!(tier2_scan.candidates.len(), 1);
    assert_eq!(tier2_scan.candidates[0].path, modular_lossy);
    let imports = foundation::fast_img::build_modern_lossy_static_import_candidates(
        dir.path(),
        &tier2_scan.candidates,
    );
    assert_eq!(imports[0].path, modular_lossy);
    assert_eq!(imports[0].blake3, tier2_scan.candidates[0].blake3);

    // A still codestream does not make a whole Live Photo a standalone image.
    let companion = modular_lossy.with_extension("MOV");
    std::fs::write(&companion, b"synthetic Live Photo companion")?;
    let live_scan =
        scan_modern_lossy_static_candidates(dir.path(), std::slice::from_ref(&modular_lossy))?;
    assert!(
        live_scan.candidates.is_empty(),
        "Tier 2 must retain Live pairs"
    );
    assert!(modular_lossy.is_file() && companion.is_file());

    // Reversible JPEG transcode: jbrd reconstruction keeps its own semantics.
    if !tool_available("magick") {
        eprintln!("Skipping jbrd evidence case: magick is unavailable to build a JPEG");
        return Ok(());
    }
    let jpeg_input = dir.path().join("input.jpg");
    let jpeg_status = Command::new("magick")
        .arg(input_arg)
        .arg(utf8(&jpeg_input)?)
        .status()?;
    anyhow::ensure!(jpeg_status.success(), "magick failed to build JPEG fixture");

    let jbrd = run_encoder(
        &dir,
        "cjxl",
        &[
            utf8(&jpeg_input)?,
            utf8(&dir.path().join("jbrd.jxl"))?,
            "--lossless_jpeg=1",
            "-e",
            "3",
        ],
        "jbrd.jxl",
    )?;
    assert_eq!(
        detect_compression(&DetectedFormat::JXL, &jbrd)?,
        CompressionType::JpegReconstruction,
        "jbrd JXL must keep JPEG-reconstruction semantics"
    );
    Ok(())
}

#[test]
fn avif_compression_evidence_from_real_avifenc_output() -> anyhow::Result<()> {
    if !tool_available("avifenc") {
        eprintln!("Skipping avif compression evidence test: avifenc is unavailable");
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let input = write_input_png(&dir)?;
    let input_arg = utf8(&input)?;

    // 4:2:0 lossy: positive chroma-subsampling evidence.
    let lossy420 = run_encoder(
        &dir,
        "avifenc",
        &[
            "-s",
            "8",
            "-y",
            "420",
            input_arg,
            utf8(&dir.path().join("y420.avif"))?,
        ],
        "y420.avif",
    )?;
    assert_eq!(
        detect_compression(&DetectedFormat::AVIF, &lossy420)?,
        CompressionType::Lossy,
        "subsampled AVIF must classify as ConfirmedLossy"
    );
    if tool_available("avifdec") {
        let tier2_scan =
            scan_modern_lossy_static_candidates(dir.path(), std::slice::from_ref(&lossy420))?;
        assert_eq!(tier2_scan.probe_failures, []);
        assert_eq!(tier2_scan.candidates.len(), 1);
        let imports = foundation::fast_img::build_modern_lossy_static_import_candidates(
            dir.path(),
            &tier2_scan.candidates,
        );
        assert_eq!(imports[0].path, lossy420);
        assert_eq!(
            imports[0].blake3,
            foundation::common_utils::calculate_blake3_hash(&lossy420)?
        );
    }

    // 4:4:4 (even at max quality): pixel format is not quantization proof.
    let y444 = run_encoder(
        &dir,
        "avifenc",
        &[
            "-s",
            "8",
            "-q",
            "100",
            "-y",
            "444",
            input_arg,
            utf8(&dir.path().join("y444.avif"))?,
        ],
        "y444.avif",
    )?;
    assert_eq!(
        detect_compression(&DetectedFormat::AVIF, &y444)?,
        CompressionType::Unknown,
        "4:4:4 AVIF has no container-level lossless/lossy proof; must stay Unknown"
    );
    let gray = dir.path().join("grayscale.png");
    image::GrayImage::from_raw(16, 16, (0..=u8::MAX).collect())
        .ok_or_else(|| anyhow::anyhow!("invalid grayscale fixture dimensions"))?
        .save(&gray)?;
    let mono = run_encoder(
        &dir,
        "avifenc",
        &[
            "--lossless",
            "-s",
            "8",
            "-y",
            "400",
            utf8(&gray)?,
            utf8(&dir.path().join("mono.avif"))?,
        ],
        "mono.avif",
    )?;
    assert_eq!(
        detect_compression(&DetectedFormat::AVIF, &mono)?,
        CompressionType::Unknown,
        "monochrome AV1 has no chroma plane to discard"
    );
    if tool_available("avifdec") {
        let scan = scan_modern_lossy_static_candidates(dir.path(), &[mono])?;
        anyhow::ensure!(
            scan.is_empty(),
            "monochrome AVIF must be retained outside the destructive lossy route"
        );
    }
    Ok(())
}

#[test]
fn fast_img_retains_animation_even_when_png_only_contains_one_timed_frame() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    for frames in [1, 2] {
        let path = dir.path().join(format!("synthetic-{frames}.png"));
        let mut encoder = png::Encoder::new(std::fs::File::create(&path)?, 1, 1);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_animated(frames, 1)?;
        let mut writer = encoder.write_header()?;
        for _ in 0..frames {
            writer.write_image_data(&[12, 34, 56, 78])?;
        }
        writer.finish()?;
        assert!(!foundation::probe_fast_img_lossless_static(&path)?);
    }
    Ok(())
}

#[test]
fn fast_img_avif_requires_lossless_color_and_alpha_frames() -> anyhow::Result<()> {
    if ["avifenc", "avifdec", "ffmpeg", "ffprobe"]
        .iter()
        .any(|tool| foundation::common_utils::resolve_tool_path(tool).is_none())
    {
        eprintln!("Skipping AVIF frame-evidence test: AVIF/FFmpeg toolchain unavailable");
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let mut image = image::RgbaImage::new(16, 16);
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        let x = u8::try_from(x * 15)?;
        let y = u8::try_from(y * 15)?;
        *pixel = image::Rgba([x, y, x ^ y, x.wrapping_add(y)]);
    }
    let rgba = dir.path().join("alpha-pattern.png");
    let rgb = dir.path().join("color-pattern.png");
    image.save(&rgba)?;
    image::DynamicImage::ImageRgba8(image)
        .to_rgb8()
        .save(&rgb)?;
    for (name, input, q, qa, expect_lossless) in [
        ("color.avif", &rgb, "100", "100", true),
        ("color-alpha.avif", &rgba, "100", "100", true),
        ("quantized-color.avif", &rgb, "45", "100", false),
        ("quantized-alpha.avif", &rgba, "100", "45", false),
    ] {
        let encoded = run_encoder(
            &dir,
            "avifenc",
            &[
                "-s",
                "8",
                "-y",
                "444",
                "--cicp",
                "1/13/0",
                "-q",
                q,
                "--qalpha",
                qa,
                utf8(input)?,
                utf8(&dir.path().join(name))?,
            ],
            name,
        )?;
        // Some older demuxers expose only the primary item. A lossless
        // fixture must remain original whenever its alpha cannot be inspected.
        let streams = Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-select_streams",
                "v",
                "-show_entries",
                "stream=id",
                "-of",
                "default=noprint_wrappers=1:nokey=1",
            ])
            .arg(&encoded)
            .output()?;
        anyhow::ensure!(streams.status.success(), "fixture stream inventory failed");
        let ids = String::from_utf8(streams.stdout)?;
        let complete = ids.lines().any(|id| id == "0x1")
            && (input == &rgb || ids.lines().any(|id| id == "0x2"));
        assert_eq!(
            foundation::probe_fast_img_lossless_static(&encoded)?,
            expect_lossless && complete,
            "{name}, item IDs {ids:?}"
        );
    }
    // Contradict the lossless frame's RGB matrix only in the container profile.
    let mut mismatched = std::fs::read(dir.path().join("color.avif"))?;
    let nclx = mismatched
        .windows(4)
        .position(|prefix| prefix == b"nclx")
        .ok_or_else(|| anyhow::anyhow!("expected synthetic nclx profile"))?;
    mismatched[nclx + 8..nclx + 10].copy_from_slice(&1u16.to_be_bytes());
    let changed = dir.path().join("matrix-conflict.avif");
    std::fs::write(&changed, mismatched)?;
    assert!(!foundation::probe_fast_img_lossless_static(&changed)?);
    Ok(())
}
