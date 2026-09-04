//! Production-facing image matrix regressions.
//!
//! The first test is dependency-free and locks the content-identity boundary.
//! The encoder tests are deliberately tool-gated: when a platform does not
//! provide the authoritative encoder/decoder, the test reports that fact
//! instead of pretending that the path was exercised.

use anyhow::{Context, Result, anyhow, ensure};
use foundation::common_utils::resolve_tool_path;
use foundation::fast_img::{verify_final_jxl_delivery_integrity, verify_jxl_roundtrip_integrity};
use foundation::image::format_detect::{FormatKind, detect_true_format};
use foundation::image_detection::{DetectedFormat, detect_animation};
use img::lossless_converter::{ConvertFlags, ConvertOptions, convert_jpeg_to_jxl, convert_to_jxl};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn tool_available(tool: &str) -> bool {
    let Some(path) = resolve_tool_path(tool) else {
        return false;
    };
    ["--help", "--version", "-version"].iter().any(|argument| {
        match Command::new(&path)
            .arg(argument)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
        {
            Ok(status) => status.success(),
            Err(error) => {
                eprintln!(
                    "image matrix tool probe failed for {} ({argument}): {error}",
                    path.display()
                );
                false
            }
        }
    })
}

fn tool_path(tool: &str) -> Result<PathBuf> {
    resolve_tool_path(tool).ok_or_else(|| anyhow!("required test tool {tool} is unavailable"))
}

fn run_status(mut command: Command, description: &str) -> Result<()> {
    let status = command
        .status()
        .with_context(|| format!("failed to launch {description}"))?;
    ensure!(
        status.success(),
        "{description} exited with {status}; test fixture was not created"
    );
    Ok(())
}

fn magick_supports_format(format: &str) -> bool {
    let Some(path) = resolve_tool_path("magick") else {
        return false;
    };
    let Ok(output) = Command::new(path).args(["-list", "format"]).output() else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let wanted = format.to_ascii_uppercase();
    String::from_utf8_lossy(&output.stdout).lines().any(|line| {
        line.split_whitespace()
            .next()
            .is_some_and(|name| name.trim_end_matches('*') == wanted)
    })
}

fn write_signature_fixture(root: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf> {
    let path = root.join(name);
    fs::write(&path, bytes)?;
    Ok(path)
}

#[test]
fn content_identity_matrix_ignores_misleading_extensions() -> Result<()> {
    let root = tempfile::tempdir()?;
    let cases: &[(&str, &[u8], FormatKind)] = &[
        ("jpeg.jpg", b"\xFF\xD8\xFF\xE0\x00\x10", FormatKind::Jpeg),
        ("png.jpg", b"\x89PNG\r\n\x1A\n\x00", FormatKind::Png),
        ("gif.jpg", b"GIF89a\x01\x00\x01\x00", FormatKind::Gif),
        ("webp.jpg", b"RIFF\x00\x00\x00\x00WEBP", FormatKind::WebP),
        ("tiff.jpg", b"II*\x00rest", FormatKind::Tiff),
        ("bmp.jpg", b"BM\x00\x00", FormatKind::Bmp),
        ("jxl.jpg", b"\xFF\x0A\x00", FormatKind::Jxl),
        (
            "avif.jpg",
            b"\x00\x00\x00\x10ftypavif\x00\x00\x00\x00",
            FormatKind::Avif,
        ),
        (
            "heic.jpg",
            b"\x00\x00\x00\x10ftypheic\x00\x00\x00\x00",
            FormatKind::Heic,
        ),
        (
            "heif.jpg",
            b"\x00\x00\x00\x14ftypmif1\x00\x00\x00\x00heif",
            FormatKind::Heif,
        ),
        ("jp2.jpg", b"\xFF\x4F\xFF\x51\x00\x00", FormatKind::Jp2),
    ];

    for (name, bytes, expected) in cases {
        let path = write_signature_fixture(root.path(), name, bytes)?;
        assert_eq!(
            detect_true_format(&path)?,
            *expected,
            "content identity must win for {name}"
        );
    }

    let truncated = write_signature_fixture(root.path(), "truncated.jpg", &[0xFF])?;
    assert_eq!(detect_true_format(&truncated)?, FormatKind::Unknown);
    let garbage = write_signature_fixture(root.path(), "garbage.jpg", &[0xA5; 64])?;
    assert_eq!(detect_true_format(&garbage)?, FormatKind::Unknown);
    Ok(())
}

fn exact_jpeg_options(output_dir: &Path) -> ConvertOptions {
    let mut options = ConvertOptions {
        output_dir: Some(output_dir.to_path_buf()),
        child_threads: 1,
        ..ConvertOptions::default()
    };
    options.flags.set(ConvertFlags::FORCE, true);
    options
        .flags
        .set(ConvertFlags::REQUIRE_OUTPUT_DELIVERY, true);
    options
        .flags
        .set(ConvertFlags::REQUIRE_JPEG_RECONSTRUCTION, true);
    options
}

fn write_jpeg_variant(path: &Path, variant: &str) -> Result<()> {
    let magick = tool_path("magick")?;
    let mut command = Command::new(magick);
    command
        .arg("-size")
        .arg("96x64")
        .arg("plasma:fractal")
        .arg("-quality")
        .arg("82");
    match variant {
        "baseline" => {
            command.arg("-interlace").arg("none");
        }
        "progressive" => {
            command.arg("-interlace").arg("Plane");
        }
        "grayscale" => {
            command.arg("-colorspace").arg("Gray");
        }
        "cmyk" => {
            command.arg("-colorspace").arg("CMYK");
        }
        _ => return Err(anyhow!("unknown JPEG fixture variant {variant}")),
    }
    command.arg(path);
    run_status(command, &format!("magick {variant} JPEG fixture"))
}

fn set_orientation(path: &Path, orientation: u8) -> Result<()> {
    let exiftool = tool_path("exiftool")?;
    let assignment = format!("-Orientation#={orientation}");
    let mut command = Command::new(exiftool);
    command.arg("-overwrite_original").arg(assignment).arg(path);
    run_status(command, "exiftool orientation fixture update")
}

fn reconstructed_jpeg(source: &Path, destination: &Path) -> Result<()> {
    foundation::image::jxl_utils::run_exact_jpeg_reconstruction(
        source,
        destination,
        "production matrix JPEG reconstruction",
    )
    .map_err(anyhow::Error::msg)?;
    ensure!(
        destination.is_file(),
        "djxl did not create {}",
        destination.display()
    );
    ensure!(
        fs::read(source)? != fs::read(destination)?,
        "JXL and JPEG containers must not be byte-identical"
    );
    Ok(())
}

const MATRIX_XMP: &[u8] = br#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:Title>production-matrix</dc:Title></rdf:Description></rdf:RDF></x:xmpmeta>"#;

fn extract_jxl_xmp(jxl: &Path, destination: &Path) -> Result<Vec<u8>> {
    let djxl = tool_path("djxl")?;
    let output = Command::new(djxl)
        .arg(jxl)
        .arg(destination)
        .arg("--output_format=xmp")
        .output()
        .with_context(|| format!("failed to launch djxl XMP extraction for {}", jxl.display()))?;
    ensure!(
        output.status.success(),
        "djxl XMP extraction failed for {}: {}",
        jxl.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    ensure!(
        destination.is_file(),
        "djxl XMP extraction did not create {}",
        destination.display()
    );
    Ok(fs::read(destination)?)
}

fn find_icc_profile() -> Option<PathBuf> {
    [
        "/System/Library/ColorSync/Profiles/Display P3.icc",
        "/System/Library/ColorSync/Profiles/DCI(P3) RGB.icc",
        "/System/Library/ColorSync/Profiles/sRGB Profile.icc",
        "/System/Library/ColorSync/Profiles/Generic RGB Profile.icc",
        "/System/Library/ColorSync/Profiles/Generic Gray Gamma 2.2 Profile.icc",
        "/Library/ColorSync/Profiles/sRGB Profile.icc",
    ]
    .iter()
    .map(PathBuf::from)
    .find(|path| path.is_file())
}

fn extract_embedded_icc(path: &Path) -> Result<Vec<u8>> {
    let output = Command::new(tool_path("exiftool")?)
        .args(["-icc_profile", "-b"])
        .arg(path)
        .output()?;
    ensure!(
        output.status.success() && !output.stdout.is_empty(),
        "{} has no readable embedded ICC profile",
        path.display()
    );
    Ok(output.stdout)
}

fn pfm_sample_bits(path: &Path) -> Result<(String, String, Vec<u32>)> {
    let bytes = fs::read(path)?;
    let mut parts = bytes.splitn(4, |byte| *byte == b'\n');
    let magic = std::str::from_utf8(parts.next().unwrap_or_default())?.to_string();
    let dimensions = std::str::from_utf8(parts.next().unwrap_or_default())?.to_string();
    let scale = std::str::from_utf8(parts.next().unwrap_or_default())?.parse::<f32>()?;
    let payload = parts.next().unwrap_or_default();
    let (sample_bytes, remainder) = payload.as_chunks::<4>();
    ensure!(
        matches!(magic.as_str(), "PF" | "Pf") && remainder.is_empty(),
        "invalid PFM payload in {}",
        path.display()
    );
    let samples = sample_bytes
        .iter()
        .map(|bytes| {
            if scale.is_sign_negative() {
                u32::from_le_bytes(*bytes)
            } else {
                u32::from_be_bytes(*bytes)
            }
        })
        .collect();
    Ok((magic, dimensions, samples))
}

fn convert_exact_jpeg_fixture(source: &Path, output_dir: &Path) -> Result<PathBuf> {
    fs::create_dir_all(output_dir)?;
    let result = convert_jpeg_to_jxl(source, &exact_jpeg_options(output_dir), None)?;
    ensure!(
        result.success && !result.skipped,
        "fixture conversion failed: {}",
        result.message
    );
    let output = PathBuf::from(
        result
            .output_path
            .as_deref()
            .ok_or_else(|| anyhow!("fixture conversion returned no output path"))?,
    );
    ensure!(
        output.is_file(),
        "fixture output is missing: {}",
        output.display()
    );
    verify_jxl_roundtrip_integrity(source, &output)?;
    verify_final_jxl_delivery_integrity(source, &output)?;
    Ok(output)
}

#[test]
fn jpeg_orientation_matrix_and_encoding_variants_are_fail_closed() -> Result<()> {
    for tool in ["magick", "cjxl", "djxl", "jxlinfo", "exiftool"] {
        if !tool_available(tool) {
            eprintln!("Skipping JPEG production matrix: {tool} is unavailable");
            return Ok(());
        }
    }

    let root = tempfile::tempdir()?;
    let input_root = root.path().join("input");
    let output_root = root.path().join("output");
    fs::create_dir_all(&input_root)?;
    fs::create_dir_all(&output_root)?;

    let baseline = input_root.join("orientation-base.jpg");
    write_jpeg_variant(&baseline, "baseline")?;
    for orientation in 1..=8 {
        let source = input_root.join(format!("orientation-{orientation}.jpg"));
        fs::copy(&baseline, &source)?;
        set_orientation(&source, orientation)?;

        let output_dir = output_root.join(format!("orientation-{orientation}"));
        fs::create_dir_all(&output_dir)?;
        let result = convert_jpeg_to_jxl(&source, &exact_jpeg_options(&output_dir), None)?;
        ensure!(
            result.success && !result.skipped,
            "orientation {orientation} was not converted: {}",
            result.message
        );
        let output = PathBuf::from(
            result
                .output_path
                .as_deref()
                .ok_or_else(|| anyhow!("orientation {orientation} returned no output path"))?,
        );
        ensure!(
            output.is_file(),
            "orientation {orientation} output is missing"
        );
        verify_jxl_roundtrip_integrity(&source, &output)?;
        verify_final_jxl_delivery_integrity(&source, &output)?;

        let reconstructed = output_dir.join(format!("orientation-{orientation}.jpg"));
        reconstructed_jpeg(&output, &reconstructed)?;
        ensure!(
            fs::read(&source)? == fs::read(&reconstructed)?,
            "orientation {orientation} reconstruction changed JPEG bytes"
        );
        let exiftool = tool_path("exiftool")?;
        let orientation_output = Command::new(exiftool)
            .arg("-s3")
            .arg("-Orientation#")
            .arg(&reconstructed)
            .output()?;
        ensure!(orientation_output.status.success());
        ensure!(
            String::from_utf8_lossy(&orientation_output.stdout).trim() == orientation.to_string(),
            "orientation {orientation} metadata was not retained"
        );
        ensure!(
            fs::read_dir(&output_dir)?
                .filter_map(std::result::Result::ok)
                .count()
                == 2,
            "orientation {orientation} output directory must contain exactly one JXL and one reconstructed JPEG"
        );
        ensure!(source.is_file(), "source JPEG was unexpectedly removed");
    }

    for (variant, must_succeed) in [
        ("baseline", true),
        ("progressive", true),
        ("grayscale", true),
        ("cmyk", false),
    ] {
        let source = input_root.join(format!("{variant}.jpg"));
        write_jpeg_variant(&source, variant)?;
        let output_dir = output_root.join(variant);
        fs::create_dir_all(&output_dir)?;
        let result = convert_jpeg_to_jxl(&source, &exact_jpeg_options(&output_dir), None);
        match result {
            Ok(result) if result.success && !result.skipped => {
                let output = PathBuf::from(
                    result
                        .output_path
                        .as_deref()
                        .ok_or_else(|| anyhow!("{variant} returned no output path"))?,
                );
                verify_jxl_roundtrip_integrity(&source, &output)?;
                verify_final_jxl_delivery_integrity(&source, &output)?;
                ensure!(
                    source.is_file(),
                    "{variant} source was unexpectedly removed"
                );
            }
            Ok(result) => {
                ensure!(
                    !must_succeed,
                    "{variant} unexpectedly failed: {}",
                    result.message
                );
                ensure!(source.is_file(), "{variant} failed path removed its source");
                ensure!(
                    fs::read_dir(&output_dir)?.next().is_none(),
                    "{variant} failed path left an unverified output"
                );
                ensure!(
                    result.message.contains("source remains")
                        || result
                            .skip_reason
                            .as_deref()
                            .is_some_and(|reason| reason.contains("source")),
                    "{variant} failure was not explicit about source retention: {}",
                    result.message
                );
            }
            Err(error) => {
                ensure!(
                    !must_succeed,
                    "{variant} returned an unexpected error: {error}"
                );
                ensure!(source.is_file(), "{variant} error path removed its source");
                ensure!(
                    fs::read_dir(&output_dir)?.next().is_none(),
                    "{variant} error path left an unverified output"
                );
            }
        }
    }
    Ok(())
}

#[test]
fn jpeg_metadata_matrix_preserves_xmp_and_icc_without_breaking_jbrd() -> Result<()> {
    for tool in ["magick", "cjxl", "djxl", "jxlinfo", "exiftool"] {
        if !tool_available(tool) {
            eprintln!("Skipping JPEG metadata matrix: {tool} is unavailable");
            return Ok(());
        }
    }

    let root = tempfile::tempdir()?;
    let input_root = root.path().join("input");
    let output_root = root.path().join("output");
    fs::create_dir_all(&input_root)?;
    fs::create_dir_all(&output_root)?;

    for with_xmp in [false, true] {
        let source = input_root.join(if with_xmp {
            "with-xmp.jpg"
        } else {
            "without-xmp.jpg"
        });
        write_jpeg_variant(&source, "baseline")?;
        let sidecar = source.with_extension("xmp");
        if with_xmp {
            fs::write(&sidecar, MATRIX_XMP)?;
        }

        let output_dir = output_root.join(if with_xmp { "with-xmp" } else { "without-xmp" });
        let output = convert_exact_jpeg_fixture(&source, &output_dir)?;
        ensure!(source.is_file(), "source JPEG was unexpectedly removed");
        ensure!(
            fs::read_dir(&output_dir)?
                .filter_map(std::result::Result::ok)
                .count()
                == 1,
            "metadata variant must leave exactly one delivered JXL"
        );

        if with_xmp {
            let extracted = extract_jxl_xmp(&output, &output_dir.join("extracted.xmp"))?;
            ensure!(
                extracted
                    .windows(b"production-matrix".len())
                    .any(|window| window == b"production-matrix"),
                "validated XMP sidecar was not present in the JXL overlay"
            );
            ensure!(
                fs::read(&sidecar)? == MATRIX_XMP,
                "source XMP sidecar changed"
            );
        } else {
            ensure!(
                !sidecar.exists(),
                "no-XMP input unexpectedly gained a sidecar"
            );
        }
    }

    let Some(profile) = find_icc_profile() else {
        eprintln!("ICC branch not executed: no system ICC profile is installed");
        return Ok(());
    };
    let source = input_root.join("with-icc.jpg");
    let mut command = Command::new(tool_path("magick")?);
    command
        .args(["-size", "96x64", "plasma:fractal", "-quality", "82"])
        .arg("-profile")
        .arg(profile)
        .arg(&source);
    run_status(command, "magick ICC JPEG fixture")?;
    let output = convert_exact_jpeg_fixture(&source, &output_root.join("with-icc"))?;
    let icc = extract_embedded_icc(&source)?;
    let output_info = Command::new(tool_path("jxlinfo")?).arg(&output).output()?;
    ensure!(output_info.status.success(), "ICC JXL feature probe failed");
    let output_info = String::from_utf8_lossy(&output_info.stdout);
    ensure!(
        output_info.contains(&format!("{}-byte ICC profile", icc.len())),
        "JXL did not advertise the source ICC profile length: {output_info}"
    );

    let reconstructed = output_root.join("with-icc-restored.jpg");
    reconstructed_jpeg(&output, &reconstructed)?;
    ensure!(
        fs::read(&reconstructed)? == fs::read(&source)?,
        "ICC JPEG was not reconstructed byte-for-byte"
    );
    let reconstructed_icc = extract_embedded_icc(&reconstructed)?;
    ensure!(
        reconstructed_icc == icc,
        "reconstructed JPEG did not preserve the source ICC profile byte-for-byte"
    );
    ensure!(source.is_file(), "ICC source JPEG was unexpectedly removed");
    ensure!(output.is_file(), "ICC JXL output is missing");
    Ok(())
}

#[test]
fn float32_display_p3_tiff_to_jxl_preserves_samples_icc_and_xmp() -> Result<()> {
    for tool in ["magick", "ffprobe", "cjxl", "djxl", "jxlinfo", "exiftool"] {
        if !tool_available(tool) {
            eprintln!("Skipping float32 Display P3 matrix: {tool} is unavailable");
            return Ok(());
        }
    }
    let Some(profile) = [
        "/System/Library/ColorSync/Profiles/Display P3.icc",
        "/System/Library/ColorSync/Profiles/DCI(P3) RGB.icc",
    ]
    .iter()
    .map(PathBuf::from)
    .find(|path| path.is_file()) else {
        eprintln!("Skipping float32 Display P3 matrix: no P3 ICC profile is installed");
        return Ok(());
    };

    let root = tempfile::tempdir()?;
    let source = root.path().join("float32-display-p3.tiff");
    let sidecar = source.with_extension("xmp");
    let output_root = root.path().join("output");
    fs::create_dir_all(&output_root)?;

    let mut command = Command::new(tool_path("magick")?);
    command
        .args([
            "-size",
            "96x64",
            "plasma:fractal",
            "-colorspace",
            "RGB",
            "-evaluate",
            "multiply",
            "0.123456789",
            "-define",
            "quantum:format=floating-point",
            "-depth",
            "32",
            "-profile",
        ])
        .arg(&profile)
        .arg(&source);
    run_status(command, "create float32 Display P3 TIFF fixture")?;
    fs::write(&sidecar, MATRIX_XMP)?;
    let source_before = fs::read(&source)?;

    let probe = Command::new(tool_path("ffprobe")?)
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=pix_fmt",
            "-of",
            "default=nw=1:nk=1",
        ])
        .arg(&source)
        .output()?;
    ensure!(
        probe.status.success() && String::from_utf8_lossy(&probe.stdout).contains("f32"),
        "fixture is not a proven 32-bit float image"
    );

    let mut options = ConvertOptions {
        output_dir: Some(output_root.clone()),
        child_threads: 1,
        ..ConvertOptions::default()
    };
    options.flags.set(ConvertFlags::FORCE, true);
    options.flags.set(ConvertFlags::ULTIMATE, true);
    let result = convert_to_jxl(&source, &options, 0.0, None)?;
    ensure!(
        result.success && !result.skipped,
        "float32 Display P3 conversion failed: {}",
        result.message
    );
    let output = PathBuf::from(
        result
            .output_path
            .ok_or_else(|| anyhow!("float32 conversion omitted output path"))?,
    );

    let output_info = Command::new(tool_path("jxlinfo")?).arg(&output).output()?;
    ensure!(
        output_info.status.success()
            && String::from_utf8_lossy(&output_info.stdout).contains("32-bit float"),
        "float32 TIFF→JXL did not retain its float sample domain"
    );

    let source_pfm = root.path().join("source.pfm");
    let decoded_pfm = root.path().join("decoded.pfm");
    let decoded_icc = root.path().join("decoded.icc");
    let mut command = Command::new(tool_path("magick")?);
    command
        .arg(&source)
        .args(["-format", "pfm"])
        .arg(&source_pfm);
    run_status(command, "create float32 source PFM proof")?;
    let mut command = Command::new(tool_path("djxl")?);
    command
        .arg(&output)
        .arg(&decoded_pfm)
        .arg(format!("--orig_icc_out={}", decoded_icc.display()));
    run_status(command, "decode float32 JXL proof")?;

    let source_samples = pfm_sample_bits(&source_pfm)?;
    let decoded_samples = pfm_sample_bits(&decoded_pfm)?;
    ensure!(
        source_samples == decoded_samples,
        "float32 TIFF→JXL changed one or more floating-point samples"
    );
    ensure!(
        source_samples.2.iter().any(|bits| {
            let sample = f32::from_bits(*bits);
            sample.is_finite() && (sample * 65_535.0).fract().abs() > 0.000_1
        }),
        "float32 fixture was accidentally representable as 16-bit integer samples"
    );
    ensure!(
        fs::read(&decoded_icc)? == extract_embedded_icc(&source)?,
        "float32 Display P3 JXL did not preserve the source ICC profile byte-for-byte"
    );
    let extracted_xmp = extract_jxl_xmp(&output, &output_root.join("float32.xmp"))?;
    ensure!(
        extracted_xmp
            .windows(b"production-matrix".len())
            .any(|window| window == b"production-matrix"),
        "float32 Display P3 JXL did not preserve its XMP overlay"
    );
    ensure!(
        fs::read(&source)? == source_before && fs::read(&sidecar)? == MATRIX_XMP,
        "float32 conversion changed its source or XMP sidecar"
    );
    Ok(())
}

fn decode_static_fixture_to_png(input: &Path, format: FormatKind, root: &Path) -> Result<PathBuf> {
    let stem = input
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or_else(|| anyhow!("fixture has no UTF-8 stem: {}", input.display()))?;
    let output = root.join(format!("{stem}-decoded.png"));
    let command = match format {
        FormatKind::Jxl => {
            let mut command = Command::new(tool_path("djxl")?);
            command.arg(input).arg(&output);
            command
        }
        FormatKind::Avif => {
            let mut command = Command::new(tool_path("avifdec")?);
            command.arg(input).arg(&output);
            command
        }
        FormatKind::Heic | FormatKind::Heif => {
            let mut command = Command::new(tool_path("heif-convert")?);
            command.arg(input).arg(&output);
            command
        }
        FormatKind::WebP => {
            let mut command = Command::new(tool_path("dwebp")?);
            command.arg(input).arg("-o").arg(&output);
            command
        }
        _ => {
            let mut command = Command::new(tool_path("magick")?);
            command.arg(input).arg(&output);
            command
        }
    };
    run_status(command, &format!("decode {} fixture", input.display()))?;
    ensure!(
        output.is_file(),
        "decoder did not create {}",
        output.display()
    );
    Ok(output)
}

/// Decode an HDR AVIF through the same `FFmpeg` high-precision path used by the
/// production JXL encoder.  `avifdec` and `FFmpeg` are both valid decoders, but
/// their RGB16 rounding can differ by one 8-bit step after a 12-bit YUV round
/// trip; comparing across decoders would turn a decoder disagreement into a
/// false loss report.
fn decode_hdr_avif_with_encoder_path(input: &Path, root: &Path) -> Result<PathBuf> {
    if !tool_available("ffmpeg") {
        eprintln!(
            "HDR AVIF reference uses avifdec because FFmpeg is unavailable; this matches the production decoder fallback"
        );
        return decode_static_fixture_to_png(input, FormatKind::Avif, root);
    }
    let output = root.join("hdr-reference-decoded.png");
    let mut command = Command::new(tool_path("ffmpeg")?);
    command
        .args(["-y", "-i"])
        .arg(input)
        .args(["-pix_fmt", "rgb48le", "-frames:v", "1"])
        .arg(&output);
    run_status(command, "FFmpeg HDR AVIF reference decode")?;
    ensure!(
        output.is_file(),
        "FFmpeg did not create HDR reference {}",
        output.display()
    );
    Ok(output)
}

fn detected_format_for_kind(kind: FormatKind) -> DetectedFormat {
    match kind {
        FormatKind::Jpeg => DetectedFormat::JPEG,
        FormatKind::Png => DetectedFormat::PNG,
        FormatKind::Heic => DetectedFormat::HEIC,
        FormatKind::Heif => DetectedFormat::HEIF,
        FormatKind::Avif => DetectedFormat::AVIF,
        FormatKind::WebP => DetectedFormat::WebP,
        FormatKind::Gif => DetectedFormat::GIF,
        FormatKind::Bmp => DetectedFormat::BMP,
        FormatKind::Jxl => DetectedFormat::JXL,
        FormatKind::Tiff => DetectedFormat::TIFF,
        FormatKind::Jp2 => DetectedFormat::JP2,
        _ => DetectedFormat::Unknown(format!("{kind:?}")),
    }
}

#[test]
fn real_static_format_matrix_is_decoded_and_extension_spoofing_is_rejected() -> Result<()> {
    if !tool_available("magick") {
        eprintln!("Skipping real static format matrix: magick is unavailable");
        return Ok(());
    }

    let root = tempfile::tempdir()?;
    let fixture_root = root.path().join("fixtures");
    let decoded_root = root.path().join("decoded");
    fs::create_dir_all(&fixture_root)?;
    fs::create_dir_all(&decoded_root)?;

    let to_byte = |value: u32| {
        u8::try_from(value % 256)
            .unwrap_or_else(|_| unreachable!("value modulo 256 always fits in a byte"))
    };
    let png = fixture_root.join("pattern.png");
    image::RgbImage::from_fn(32, 24, |x, y| {
        image::Rgb([
            to_byte(x * 7 + 11),
            to_byte(y * 9 + 17),
            to_byte((x + y) * 5 + 23),
        ])
    })
    .save(&png)?;

    let mut cases = vec![(png.clone(), FormatKind::Png)];
    for extension in ["tiff", "webp", "heic"] {
        if !magick_supports_format(&extension.to_ascii_uppercase()) {
            eprintln!("{extension} branch not executed: ImageMagick delegate is unavailable");
            continue;
        }
        let path = fixture_root.join(format!("pattern.{extension}"));
        let mut command = Command::new(tool_path("magick")?);
        command.arg(&png).arg(&path);
        run_status(command, &format!("magick {extension} fixture"))?;
        cases.push((
            path,
            match extension {
                "tiff" => FormatKind::Tiff,
                "webp" => FormatKind::WebP,
                "heic" => FormatKind::Heic,
                _ => unreachable!("extension is fixed above"),
            },
        ));
    }

    let gif = fixture_root.join("pattern.gif");
    if magick_supports_format("GIF") {
        let mut gif_command = Command::new(tool_path("magick")?);
        gif_command.arg(&png).arg(&gif);
        run_status(gif_command, "magick static GIF fixture")?;
        cases.push((gif, FormatKind::Gif));
    } else {
        eprintln!("GIF branch not executed: ImageMagick delegate is unavailable");
    }

    if tool_available("avifenc") {
        let avif = fixture_root.join("pattern.avif");
        let mut command = Command::new(tool_path("avifenc")?);
        command.args(["-s", "8", "-y", "420"]).arg(&png).arg(&avif);
        run_status(command, "avifenc static fixture")?;
        cases.push((avif, FormatKind::Avif));
    } else {
        eprintln!("AVIF branch not executed: avifenc is unavailable");
    }

    if tool_available("cjxl") {
        let jxl = fixture_root.join("pattern.jxl");
        let mut command = Command::new(tool_path("cjxl")?);
        command.args(["-d", "0", "-e", "3"]).arg(&png).arg(&jxl);
        run_status(command, "cjxl static fixture")?;
        cases.push((jxl, FormatKind::Jxl));
    } else {
        eprintln!("JXL branch not executed: cjxl is unavailable");
    }

    for (path, expected) in cases {
        let before = fs::read(&path)?;
        assert_eq!(detect_true_format(&path)?, expected, "{}", path.display());
        let disguised = path.with_file_name(format!(
            "{}.jpg",
            path.file_stem()
                .and_then(|value| value.to_str())
                .ok_or_else(|| anyhow!("fixture has no UTF-8 stem"))?
        ));
        fs::copy(&path, &disguised)?;
        assert_eq!(
            detect_true_format(&disguised)?,
            expected,
            "content identity must ignore the .jpg suffix for {}",
            path.display()
        );
        let detected = detected_format_for_kind(expected);
        let (animated, frame_count, _) = detect_animation(&path, &detected)?;
        ensure!(
            !animated,
            "static fixture was classified as animated: {}",
            path.display()
        );
        ensure!(
            frame_count.is_none_or(|count| count <= 1),
            "static fixture reported multiple frames: {} ({frame_count:?})",
            path.display()
        );

        let decoded = if expected == FormatKind::Avif && !tool_available("avifdec") {
            eprintln!("AVIF pixel branch not executed: avifdec is unavailable");
            None
        } else if matches!(expected, FormatKind::Heic | FormatKind::Heif)
            && !tool_available("heif-convert")
        {
            eprintln!("HEIC pixel branch not executed: heif-convert is unavailable");
            None
        } else if expected == FormatKind::WebP && !tool_available("dwebp") {
            eprintln!("WebP pixel branch not executed: dwebp is unavailable");
            None
        } else if expected == FormatKind::Jxl && !tool_available("djxl") {
            eprintln!("JXL pixel branch not executed: djxl is unavailable");
            None
        } else {
            Some(decode_static_fixture_to_png(
                &path,
                expected,
                &decoded_root,
            )?)
        };
        if let Some(decoded) = decoded {
            let image = foundation::image_detection::open_image_with_limits(&decoded)?;
            ensure!(
                (image.width(), image.height()) == (32, 24),
                "decoded {} dimensions changed to {}x{}",
                path.display(),
                image.width(),
                image.height()
            );
            let pixels = image.to_rgb8();
            ensure!(
                pixels.as_raw().iter().any(|channel| *channel != 0),
                "decoded {} unexpectedly contains only black pixels",
                path.display()
            );
        }
        ensure!(
            fs::read(&path)? == before,
            "format probe changed source {}",
            path.display()
        );
    }
    Ok(())
}

fn add_magick_lossless_fixtures(
    fixture_root: &Path,
    png: &Path,
    fixtures: &mut Vec<(PathBuf, FormatKind)>,
) -> Result<()> {
    for (extension, format) in [
        ("bmp", FormatKind::Bmp),
        ("tiff", FormatKind::Tiff),
        ("tga", FormatKind::Unknown),
        ("ico", FormatKind::Ico),
        ("cur", FormatKind::Ico),
        ("pnm", FormatKind::Pnm),
        ("pam", FormatKind::Pnm),
    ] {
        if !magick_supports_format(&extension.to_ascii_uppercase()) {
            eprintln!("{extension} lossless JXL branch not executed: delegate unavailable");
            continue;
        }
        let source = fixture_root.join(format!("pattern.{extension}"));
        let mut command = Command::new(tool_path("magick")?);
        command.arg(png).arg(&source);
        run_status(command, &format!("create lossless {extension} fixture"))?;
        fs::write(source.with_extension("xmp"), MATRIX_XMP)?;
        fixtures.push((source, format));
    }
    if magick_supports_format("WEBP") {
        let source = fixture_root.join("pattern.webp");
        let mut command = Command::new(tool_path("magick")?);
        command
            .arg(png)
            .args(["-define", "webp:lossless=true"])
            .arg(&source);
        run_status(command, "create lossless WebP fixture")?;
        fs::write(source.with_extension("xmp"), MATRIX_XMP)?;
        fixtures.push((source, FormatKind::WebP));
    }
    Ok(())
}

const fn low_u16(value: u32) -> u16 {
    let bytes = value.to_le_bytes();
    u16::from_le_bytes([bytes[0], bytes[1]])
}

fn add_avif_lossless_fixture(
    fixture_root: &Path,
    png: &Path,
    fixtures: &mut Vec<(PathBuf, FormatKind)>,
) -> Result<()> {
    if tool_available("avifenc") && tool_available("avifdec") {
        let source = fixture_root.join("pattern.avif");
        let mut command = Command::new(tool_path("avifenc")?);
        command
            .args(["--lossless", "--speed", "8"])
            .arg(png)
            .arg(&source);
        run_status(command, "create lossless AVIF fixture")?;
        fs::write(source.with_extension("xmp"), MATRIX_XMP)?;
        fixtures.push((source, FormatKind::Avif));

        let hdr_png = fixture_root.join("pattern-hdr.png");
        image::ImageBuffer::<image::Rgb<u16>, Vec<u16>>::from_fn(96, 64, |x, y| {
            let red = (x * 521 + y * 257) % 65_536;
            let green = (x * 313 + y * 733 + 17) % 65_536;
            let blue = (x * 911 + y * 419 + 31) % 65_536;
            image::Rgb([low_u16(red), low_u16(green), low_u16(blue)])
        })
        .save(&hdr_png)?;
        let hdr_source = fixture_root.join("pattern-hdr.avif");
        let mut command = Command::new(tool_path("avifenc")?);
        command
            .args([
                "--lossless",
                "--speed",
                "8",
                "--depth",
                "12,4",
                "--yuv",
                "444",
                "--cicp",
                "9/16/0",
                "--clli",
                "1000,400",
            ])
            .arg(&hdr_png)
            .arg(&hdr_source);
        run_status(command, "create lossless HDR AVIF fixture")?;
        fs::write(hdr_source.with_extension("xmp"), MATRIX_XMP)?;
        fixtures.push((hdr_source, FormatKind::Avif));
    }
    Ok(())
}

fn create_lossless_raster_fixtures(root: &Path) -> Result<Vec<(PathBuf, FormatKind)>> {
    let fixture_root = root.join("lossless-input");
    fs::create_dir_all(&fixture_root)?;
    let png = fixture_root.join("pattern.png");
    image::RgbImage::from_fn(96, 64, |x, y| {
        image::Rgb([
            (x * 5 + y * 3).to_le_bytes()[0],
            (x * 7 + y * 11).to_le_bytes()[0],
            (x * 13 + y * 17).to_le_bytes()[0],
        ])
    })
    .save(&png)?;
    fs::write(png.with_extension("xmp"), MATRIX_XMP)?;

    let mut fixtures = vec![(png.clone(), FormatKind::Png)];
    let high_bit_depth =
        image::ImageBuffer::<image::Rgba<u16>, Vec<u16>>::from_fn(96, 64, |x, y| {
            let red = (x * 1021 + y * 4093 + 3) % 65_536;
            let green = (x * 2053 + y * 8191 + 5) % 65_536;
            let blue = (x * 4099 + y * 12289 + 7) % 65_536;
            let alpha = (x * 631 + y * 997 + 12_345) % 65_535 + 1;
            image::Rgba([low_u16(red), low_u16(green), low_u16(blue), low_u16(alpha)])
        });
    let high_bit_png = fixture_root.join("pattern-16.png");
    high_bit_depth.save(&high_bit_png)?;
    fs::write(high_bit_png.with_extension("xmp"), MATRIX_XMP)?;
    fixtures.push((high_bit_png, FormatKind::Png));
    if magick_supports_format("TIFF") {
        let high_bit_tiff = fixture_root.join("pattern-16.tiff");
        let mut command = Command::new(tool_path("magick")?);
        command
            .arg(fixture_root.join("pattern-16.png"))
            .args(["-depth", "16", "-define", "tiff:alpha=unassociated"])
            .arg(&high_bit_tiff);
        run_status(command, "create standards-valid 16-bit RGBA TIFF fixture")?;
        let mut command = Command::new(tool_path("exiftool")?);
        command
            .args([
                "-overwrite_original",
                "-XResolution=300",
                "-YResolution=150",
                "-ResolutionUnit#=2",
            ])
            .arg(&high_bit_tiff);
        run_status(command, "set TIFF print-resolution fixture metadata")?;
        fs::write(high_bit_tiff.with_extension("xmp"), MATRIX_XMP)?;
        fixtures.push((high_bit_tiff, FormatKind::Tiff));
    }
    add_magick_lossless_fixtures(&fixture_root, &png, &mut fixtures)?;
    add_avif_lossless_fixture(&fixture_root, &png, &mut fixtures)?;
    Ok(fixtures)
}

fn verify_lossless_raster_jxl_case(root: &Path, source: &Path, format: FormatKind) -> Result<()> {
    let source_before = fs::read(source)?;
    let extension = source
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("unknown");
    let output_root = root.join(format!("lossless-output-{extension}"));
    fs::create_dir_all(&output_root)?;
    let mut options = ConvertOptions {
        output_dir: Some(output_root.clone()),
        child_threads: 1,
        ..ConvertOptions::default()
    };
    options.flags.set(ConvertFlags::FORCE, true);
    options.flags.set(ConvertFlags::ULTIMATE, true);

    let result = convert_to_jxl(source, &options, 0.0, None)?;
    ensure!(
        result.success && !result.skipped,
        "lossless {format:?} conversion did not complete: {}",
        result.message
    );
    let output = PathBuf::from(
        result
            .output_path
            .ok_or_else(|| anyhow!("lossless {format:?} conversion omitted output path"))?,
    );
    ensure!(
        detect_true_format(&output)? == FormatKind::Jxl,
        "lossless {format:?} output is not JXL"
    );
    foundation::jxl_utils::verify_jxl_health(&output)
        .map_err(|error| anyhow!("lossless {format:?} JXL health failed: {error}"))?;

    let source_decoded_root = output_root.join("source-decoded");
    let output_decoded_root = output_root.join("output-decoded");
    fs::create_dir_all(&source_decoded_root)?;
    fs::create_dir_all(&output_decoded_root)?;
    let is_hdr = source
        .file_stem()
        .and_then(|value| value.to_str())
        .is_some_and(|stem| stem.ends_with("-hdr"));
    let is_high_bit_depth = source
        .file_stem()
        .and_then(|value| value.to_str())
        .is_some_and(|stem| stem.ends_with("-16"));
    let source_decoded = if is_hdr {
        decode_hdr_avif_with_encoder_path(source, &source_decoded_root)?
    } else {
        decode_static_fixture_to_png(source, format, &source_decoded_root)?
    };
    let output_decoded =
        decode_static_fixture_to_png(&output, FormatKind::Jxl, &output_decoded_root)?;
    let source_pixels =
        foundation::image_detection::open_image_with_limits(&source_decoded)?.to_rgba16();
    let output_pixels =
        foundation::image_detection::open_image_with_limits(&output_decoded)?.to_rgba16();
    if is_hdr {
        let source_image = foundation::image_detection::open_image_with_limits(&source_decoded)?;
        let output_image = foundation::image_detection::open_image_with_limits(&output_decoded)?;
        ensure!(
            source_image.color().bits_per_pixel() >= 48
                && output_image.color().bits_per_pixel() >= 48,
            "HDR AVIF→JXL path reduced the 16-bit RGB precision"
        );
        ensure!(
            source_pixels.as_raw().iter().any(|channel| *channel > 4096),
            "HDR fixture did not contain meaningful high-range samples"
        );
    }
    if is_high_bit_depth {
        let source_image = foundation::image_detection::open_image_with_limits(&source_decoded)?;
        let output_image = foundation::image_detection::open_image_with_limits(&output_decoded)?;
        ensure!(
            source_image.color().bits_per_pixel() >= 64
                && output_image.color().bits_per_pixel() >= 64,
            "high-bit-depth {format:?}→JXL path reduced 16-bit RGBA precision"
        );
        ensure!(
            source_pixels
                .as_raw()
                .iter()
                .any(|channel| *channel % 257 != 0),
            "high-bit-depth fixture was accidentally representable as 8-bit RGB"
        );
        ensure!(
            source_pixels
                .enumerate_pixels()
                .any(|(_, _, pixel)| !matches!(pixel.0[3], 0 | u16::MAX)),
            "high-bit-depth fixture did not exercise nontrivial alpha"
        );
    }
    ensure!(
        source_pixels.dimensions() == output_pixels.dimensions()
            && source_pixels.as_raw() == output_pixels.as_raw(),
        "lossless {format:?} JXL changed decoded pixels"
    );
    ensure!(
        fs::read(source)? == source_before,
        "lossless {format:?} conversion changed its source"
    );

    let extracted = extract_jxl_xmp(&output, &output_root.join("extracted.xmp"))?;
    ensure!(
        extracted
            .windows(b"production-matrix".len())
            .any(|window| window == b"production-matrix"),
        "lossless {format:?} JXL did not preserve its adjacent XMP"
    );
    ensure!(
        fs::read(source.with_extension("xmp"))? == MATRIX_XMP,
        "lossless {format:?} source XMP changed"
    );

    if is_high_bit_depth && matches!(format, FormatKind::Tiff) {
        ensure!(
            read_numeric_print_resolution(source)? == (300.0, 150.0, 2),
            "TIFF fixture did not retain its 300x150 dpi source contract"
        );
        ensure!(
            read_numeric_print_resolution(&output)? == (300.0, 150.0, 2),
            "TIFF→JXL did not preserve print resolution and unit"
        );
    }

    if is_hdr {
        let source_info = Command::new(tool_path("avifdec")?)
            .arg("--info")
            .arg(source)
            .output()?;
        ensure!(
            source_info.status.success(),
            "HDR AVIF fixture probe failed"
        );
        let source_info = String::from_utf8_lossy(&source_info.stdout);
        ensure!(
            source_info.contains("Color Primaries: 9")
                && source_info.contains("Transfer Char. : 16")
                && source_info.contains("CLLI           : 1000, 400"),
            "HDR AVIF fixture lost its Rec.2100/PQ/CLLI contract: {source_info}"
        );

        let output_info = Command::new(tool_path("jxlinfo")?).arg(&output).output()?;
        ensure!(output_info.status.success(), "HDR JXL feature probe failed");
        let output_info = String::from_utf8_lossy(&output_info.stdout);
        ensure!(
            output_info.contains("Primaries: Rec.2100")
                && output_info.contains("Transfer function: PQ"),
            "lossless HDR AVIF→JXL lost Rec.2100/PQ signaling: {output_info}"
        );
    }
    Ok(())
}

fn read_numeric_print_resolution(path: &Path) -> Result<(f64, f64, u64)> {
    let output = Command::new(tool_path("exiftool")?)
        .args([
            "-s3",
            "-n",
            "-XResolution",
            "-YResolution",
            "-ResolutionUnit",
        ])
        .arg(path)
        .output()?;
    ensure!(
        output.status.success(),
        "ExifTool print-resolution probe failed for {}: {}",
        path.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    let text = std::str::from_utf8(&output.stdout)?;
    let mut values = text.lines();
    let x = values
        .next()
        .ok_or_else(|| anyhow!("XResolution is unavailable"))?
        .parse::<f64>()?;
    let y = values
        .next()
        .ok_or_else(|| anyhow!("YResolution is unavailable"))?
        .parse::<f64>()?;
    let unit = values
        .next()
        .ok_or_else(|| anyhow!("ResolutionUnit is unavailable"))?
        .parse::<u64>()?;
    Ok((x, y, unit))
}

#[test]
fn lossless_static_to_jxl_matrix_is_pixel_exact_and_preserves_xmp() -> Result<()> {
    if let Some(tool) = ["cjxl", "djxl", "jxlinfo", "exiftool", "magick"]
        .into_iter()
        .find(|tool| !tool_available(tool))
    {
        eprintln!("Skipping lossless raster JXL matrix: {tool} is unavailable");
        return Ok(());
    }

    let root = tempfile::tempdir()?;
    for (source, format) in create_lossless_raster_fixtures(root.path())? {
        verify_lossless_raster_jxl_case(root.path(), &source, format)?;
    }
    Ok(())
}

#[test]
fn truncated_jpeg_with_xmp_fails_closed_without_source_or_output_mutation() -> Result<()> {
    let root = tempfile::tempdir()?;
    let input = root.path().join("input");
    let output = root.path().join("output");
    fs::create_dir_all(&input)?;
    let source = input.join("broken.jpg");
    let sidecar = input.join("broken.xmp");
    let source_bytes = [0xFF, 0xD8, 0xFF, 0xE1, 0x00, 0x04];
    fs::write(&source, source_bytes)?;
    fs::write(&sidecar, MATRIX_XMP)?;

    let result = convert_jpeg_to_jxl(&source, &exact_jpeg_options(&output), None)?;
    ensure!(
        !result.success,
        "truncated JPEG must not be reported as success"
    );
    ensure!(
        !result.skipped,
        "a malformed JPEG is a failure, not a benign skip"
    );
    ensure!(
        result
            .message
            .contains("JPEG cannot be byte-identically reconstructed"),
        "failure must state the exact reconstruction boundary: {}",
        result.message
    );
    ensure!(
        fs::read(&source)? == source_bytes,
        "source JPEG was changed"
    );
    ensure!(
        fs::read(&sidecar)? == MATRIX_XMP,
        "source XMP sidecar was changed"
    );
    ensure!(
        !output.exists() || fs::read_dir(&output)?.next().is_none(),
        "failed conversion left an unverified output"
    );
    Ok(())
}

fn synthetic_animated_webp() -> Result<Vec<u8>> {
    fn anmf(duration_ms: u32) -> Result<Vec<u8>> {
        let mut payload = vec![0u8; 16];
        payload[12] = u8::try_from(duration_ms & 0xff).context("duration low byte")?;
        payload[13] = u8::try_from((duration_ms >> 8) & 0xff).context("duration middle byte")?;
        payload[14] = u8::try_from((duration_ms >> 16) & 0xff).context("duration high byte")?;
        payload.extend_from_slice(b"VP8L\x00\x00\x00\x00");
        let size = u32::try_from(payload.len()).context("synthetic WebP payload fits u32")?;
        let mut chunk = b"ANMF".to_vec();
        chunk.extend_from_slice(&size.to_le_bytes());
        chunk.extend(payload);
        if !chunk.len().is_multiple_of(2) {
            chunk.push(0);
        }
        Ok(chunk)
    }

    let vp8x = [
        b'V', b'P', b'8', b'X', 10, 0, 0, 0, 0x02, 0, 0, 0, 15, 0, 0, 15, 0, 0,
    ];
    let anim = [b'A', b'N', b'I', b'M', 6, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    let mut body = Vec::new();
    body.extend_from_slice(&vp8x);
    body.extend_from_slice(&anim);
    body.extend(anmf(100)?);
    body.extend(anmf(200)?);

    let riff_size = u32::try_from(body.len() + 4).context("synthetic WebP body fits u32")?;
    let mut output = b"RIFF".to_vec();
    output.extend_from_slice(&riff_size.to_le_bytes());
    output.extend_from_slice(b"WEBP");
    output.extend(body);
    Ok(output)
}

#[test]
fn animated_webp_is_not_admitted_as_a_static_image() -> Result<()> {
    let root = tempfile::tempdir()?;
    let path = root.path().join("animation.jpg");
    fs::write(&path, synthetic_animated_webp()?)?;
    ensure!(
        detect_true_format(&path)? == FormatKind::WebP,
        "content identity must override the misleading .jpg extension"
    );
    let (animated, frame_count, _) = detect_animation(&path, &DetectedFormat::WebP)?;
    ensure!(
        animated,
        "two-frame WebP must remain in the animated domain"
    );
    ensure!(frame_count.is_some_and(|count| count == 2));
    ensure!(
        foundation::image_detection::detect_compression(&DetectedFormat::WebP, &path)?
            == foundation::image_detection::CompressionType::Lossless,
        "all-VP8L animation frames should retain their lossless classification"
    );
    Ok(())
}

fn write_ftyp_fixture(path: &Path, major: [u8; 4], compatible: &[[u8; 4]]) -> Result<()> {
    let payload_len = 8usize
        .checked_add(
            compatible
                .len()
                .checked_mul(4)
                .context("brand count overflow")?,
        )
        .context("ftyp payload length overflow")?;
    let box_size = u32::try_from(payload_len + 8).context("ftyp box is too large")?;
    let mut bytes = Vec::with_capacity(payload_len + 8);
    bytes.extend_from_slice(&box_size.to_be_bytes());
    bytes.extend_from_slice(b"ftyp");
    bytes.extend_from_slice(&major);
    bytes.extend_from_slice(&0u32.to_be_bytes());
    for brand in compatible {
        bytes.extend_from_slice(brand);
    }
    fs::write(path, bytes)?;
    Ok(())
}

#[test]
fn isobmff_sequence_brands_are_explicitly_classified() -> Result<()> {
    let root = tempfile::tempdir()?;
    let avif_sequence = root.path().join("sequence.avif");
    write_ftyp_fixture(&avif_sequence, *b"avif", &[*b"avis"])?;
    ensure!(
        foundation::image_detection::is_isobmff_animated_sequence(&avif_sequence)?,
        "avis compatible brand must identify an AVIF sequence"
    );

    let heic_sequence = root.path().join("sequence.heic");
    write_ftyp_fixture(&heic_sequence, *b"msf1", &[])?;
    ensure!(
        foundation::image_detection::is_isobmff_animated_sequence(&heic_sequence)?,
        "msf1 major brand must identify a multi-sample sequence"
    );

    let still = root.path().join("still.avif");
    write_ftyp_fixture(&still, *b"avif", &[])?;
    ensure!(
        !foundation::image_detection::is_isobmff_animated_sequence(&still)?,
        "ordinary avif must not be classified as an animation from its brand alone"
    );
    Ok(())
}

#[test]
fn tier2_cleanup_prunes_only_verified_source_directories() -> Result<()> {
    use foundation::pipeline::verification::LibraryAssetRecord;

    let root = tempfile::tempdir()?;
    let source_root = root.path().join("source");
    let delivered = source_root.join("album/day");
    let unrelated = source_root.join("manual");
    fs::create_dir_all(&delivered)?;
    fs::create_dir_all(&unrelated)?;
    fs::write(
        delivered.join(".DS_Store"),
        [0, 0, 0, 1, b'B', b'u', b'd', b'1', 0],
    )?;
    fs::write(unrelated.join(".hidden-user-file"), b"keep")?;

    let imported = [LibraryAssetRecord {
        rel_path: "album/day/photo.webp".to_string(),
        blake3: "proof".to_string(),
        sync_status: "synced".to_string(),
        quarantined: false,
        photos_uuid: Some("uuid".to_string()),
        library_blake3: None,
        xmp_sidecar_blake3: None,
    }];
    let pruned =
        foundation::prune_empty_source_dirs_for_tier2_assets(&source_root, &imported, true)?;
    ensure!(pruned == 2, "leaf and empty parent should be pruned");
    ensure!(!source_root.join("album").exists());
    ensure!(unrelated.join(".hidden-user-file").is_file());
    ensure!(
        source_root.is_dir(),
        "unrelated content keeps the selected root"
    );
    Ok(())
}

#[test]
fn generated_static_and_animated_inputs_keep_domain_boundaries() -> Result<()> {
    if !tool_available("magick") {
        eprintln!("Skipping generated image-domain matrix: magick is unavailable");
        return Ok(());
    }
    if !magick_supports_format("GIF") {
        eprintln!("Skipping generated image-domain matrix: GIF delegate is unavailable");
        return Ok(());
    }
    let root = tempfile::tempdir()?;
    let png = root.path().join("still.png");
    run_status(
        {
            let magick = tool_path("magick")?;
            let mut command = Command::new(magick);
            command.args(["-size", "32x24", "plasma:fractal"]).arg(&png);
            command
        },
        "magick PNG fixture",
    )?;
    let disguised = root.path().join("still.jpg");
    fs::copy(&png, &disguised)?;
    assert_eq!(detect_true_format(&disguised)?, FormatKind::Png);
    assert_eq!(
        detect_animation(&disguised, &DetectedFormat::PNG)?,
        (false, None, None)
    );

    let animated = root.path().join("animated.gif");
    run_status(
        {
            let magick = tool_path("magick")?;
            let mut command = Command::new(magick);
            command
                .args(["-size", "16x16", "xc:red", "-size", "16x16", "xc:blue"])
                .arg(&animated);
            command
        },
        "magick animated GIF fixture",
    )?;
    assert_eq!(detect_true_format(&animated)?, FormatKind::Gif);
    let (is_animated, frame_count, _) = detect_animation(&animated, &DetectedFormat::GIF)?;
    ensure!(
        is_animated,
        "two-frame GIF must stay in the animated domain"
    );
    ensure!(frame_count.is_some_and(|count| count >= 2));
    Ok(())
}
