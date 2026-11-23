//! Fast IMG direct pixel encoding for sources with proven lossless compression.
use super::{
    ConvertFlags, ConvertOptions, decode_avif_for_cjxl, ensure_avif_jxl_metadata_preservable,
    jpeg_encode_threads, run_image_process,
};
use anyhow::{Context, Result, ensure};
use foundation::ToolBuilder;
use foundation::image::format_detect::{FormatKind, detect_regular_file_format};
use foundation::image::orientation::{
    PixelDiffResult, read_exif_orientation, verify_lossless_png_rasters,
};
use std::fs::{self, File};
use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;

fn scratch(suffix: &str) -> std::io::Result<NamedTempFile> {
    foundation::media_conversion_gate::delivery_named_tempfile_in_scratch_or_err(
        "fast_lossless_pixel_jxl",
        None,
        Some(suffix),
    )
}

fn png_header(path: &Path) -> Result<png::Info<'static>> {
    let mut options = png::DecodeOptions::default();
    options.set_ignore_text_chunk(true);
    options.set_skip_ancillary_crc_failures(false);
    let decoder = png::Decoder::new_with_options(BufReader::new(File::open(path)?), options);
    let info = decoder.read_info()?.info().clone();
    // A permissive raster decoder may discard malformed ancillary color chunks,
    // notably a broken deflate stream in iCCP. Absence must be proven separately.
    let lost_declaration =
        foundation::image::png_validation::has_matching_chunk(path, |kind| match kind {
            b"iCCP" => info.icc_profile.is_none(),
            b"cICP" => info.coding_independent_code_points.is_none(),
            b"sRGB" => info.srgb.is_none(),
            b"cHRM" => info.chrm_chunk.is_none(),
            b"gAMA" => info.gama_chunk.is_none(),
            b"mDCV" => info.mastering_display_color_volume.is_none(),
            b"cLLI" => info.content_light_level.is_none(),
            _ => false,
        })?;
    ensure!(
        !lost_declaration,
        "could not read every declared PNG color field"
    );
    Ok(info)
}

// The PNG reference keeps its native integer precision. avifdec uses its sample-transform
// aware 16-bit mode. dwebp exports all VP8L channels but omits the source ICC.
struct SourceRaster {
    path: PathBuf,
    _temp_png: Option<NamedTempFile>,
    color: png::Info<'static>,
}

impl SourceRaster {
    fn read(source: &Path) -> Result<Self> {
        let format = detect_regular_file_format(source)?;
        let (path, temp) = match format {
            FormatKind::Png => (source.to_owned(), None),
            FormatKind::WebP => {
                let png = scratch(".png")?;
                let process = run_image_process(
                    foundation::image_builders::DwebpBuilder::new()
                        .input(source)
                        .output(png.path())
                        .build(),
                )?;
                ensure!(
                    process.status.success(),
                    "dwebp could not decode the lossless source: {}",
                    String::from_utf8_lossy(&process.stderr).trim()
                );
                (png.path().to_owned(), Some(png))
            }
            FormatKind::Avif => {
                ensure_avif_jxl_metadata_preservable(source)?;
                decode_avif_for_cjxl(source)?
            }
            _ => anyhow::bail!("unsupported lossless pixel source: {format:?}"),
        };
        let mut color = png_header(&path)?;
        if format == FormatKind::WebP {
            use image::ImageDecoder;
            let mut decoder =
                image::codecs::webp::WebPDecoder::new(BufReader::new(File::open(source)?))?;
            color.icc_profile = decoder.icc_profile()?.map(Into::into);
        } else if format == FormatKind::Avif {
            let capture = run_image_process(
                foundation::ExiftoolBuilder::new()
                    .extract_icc_profile()
                    .input(source)
                    .build(),
            )?;
            ensure!(
                capture.status.success() && capture.stderr.is_empty(),
                "could not establish the original color profile"
            );
            if !capture.stdout.is_empty() {
                color.icc_profile = Some(capture.stdout.into());
            }
        }
        Ok(Self {
            path,
            _temp_png: temp,
            color,
        })
    }

    fn check_color(
        &self,
        output: &png::Info<'_>,
        original_icc: Option<&Path>,
        codestream_cicp: Option<[u8; 4]>,
    ) -> Result<()> {
        let source = &self.color;
        if let Some(profile) = source.icc_profile.as_deref() {
            ensure!(
                !profile.is_empty()
                    && fs::read(original_icc.context("missing JXL ICC witness")?)? == profile,
                "JXL original ICC profile differs from the source"
            );
        }
        if let Some(cicp) = source.coding_independent_code_points {
            let expected = [
                cicp.color_primaries,
                cicp.transfer_function,
                cicp.matrix_coefficients,
                u8::from(cicp.is_video_full_range_image),
            ];
            ensure!(
                output.coding_independent_code_points == Some(cicp)
                    || codestream_cicp == Some(expected),
                "JXL decoded color space does not match source CICP"
            );
        }
        if let Some(intent) = source.srgb {
            ensure!(
                output.srgb == Some(intent),
                "JXL sRGB rendering intent changed"
            );
        }
        ensure!(
            source
                .source_gamma
                .is_none_or(|g| output.source_gamma == Some(g))
                && source
                    .source_chromaticities
                    .is_none_or(|c| output.source_chromaticities == Some(c))
                && source
                    .mastering_display_color_volume
                    .is_none_or(|v| output.mastering_display_color_volume == Some(v))
                && source
                    .content_light_level
                    .is_none_or(|l| output.content_light_level == Some(l)),
            "JXL lost a source gamma, chromaticity or HDR declaration"
        );
        if source.icc_profile.is_none()
            && source.coding_independent_code_points.is_none()
            && source.chrm_chunk.is_none()
            && source.gama_chunk.is_none()
            && source.srgb.is_none()
        {
            ensure!(
                output.srgb.is_some(),
                "untagged source requires the default sRGB output"
            );
        }
        Ok(())
    }

    fn verify(&self, source: &Path, candidate: &Path) -> Result<()> {
        ensure!(
            detect_regular_file_format(candidate)? == FormatKind::Jxl,
            "expected a JXL candidate"
        );
        let cicp = foundation::image_formats::jxl::static_pixel_color_encoding(candidate)?;
        let decoded = scratch(".png")?;
        let icc = self
            .color
            .icc_profile
            .as_ref()
            .map(|_| scratch(".icc"))
            .transpose()?;
        let mut command = foundation::DjxlBuilder::new()
            .input(candidate)
            .output(decoded.path())
            .build();
        if let Some(icc) = &icc {
            command.arg("--orig_icc_out").arg(icc.path());
        }
        let process = run_image_process(command)?;
        ensure!(
            process.status.success(),
            "djxl pixel proof failed: {}",
            String::from_utf8_lossy(&process.stderr).trim()
        );
        self.check_color(
            &png_header(decoded.path())?,
            icc.as_ref().map(NamedTempFile::path),
            cicp,
        )?;
        let proof = verify_lossless_png_rasters(source, &self.path, decoded.path())?;
        ensure!(
            proof == PixelDiffResult::Match,
            "lossless source/JXL samples differ: {proof:?}"
        );
        Ok(())
    }
}

/// Independently decode the source and the final JXL, checking displayed sample and color identity.
/// # Errors
/// Rejects unavailable, incomplete or different raster and color evidence.
pub fn verify_fast_lossless_jxl_pixels(source: &Path, output: &Path) -> Result<()> {
    SourceRaster::read(source)?.verify(source, output)
}

/// Compare complete file sizes after final metadata delivery; an empty file is never eligible.
#[must_use]
pub const fn fast_lossless_jxl_fits(source_bytes: u64, finalized_bytes: u64) -> bool {
    finalized_bytes > 0 && finalized_bytes <= source_bytes
}

/// Encode the authoritative input raster at fixed d=0, exploring only compression effort.
///
/// `staged_output` is a private caller-owned staging path. Publication and the final
/// metadata-inclusive size decision belong to the delivery transaction.
/// # Errors
/// Rejects unproven sources, encoder failures, drift, or a failed sample/color proof.
pub fn encode_fast_lossless_jxl(
    source: &Path,
    staged_output: &Path,
    options: &ConvertOptions,
) -> Result<()> {
    let before = foundation::common_utils::calculate_blake3_hash(source)?;
    ensure!(
        foundation::probe_fast_img_lossless_static(source)?,
        "source lacks static lossless evidence"
    );
    if staged_output.try_exists()? {
        ensure!(
            fs::symlink_metadata(staged_output)?.file_type().is_file()
                && fs::metadata(staged_output)?.len() == 0,
            "lossless JXL staging must be empty"
        );
    }
    let raster = SourceRaster::read(source)?;
    let orientation = read_exif_orientation(source)?;
    let mut icc = raster
        .color
        .icc_profile
        .as_ref()
        .map(|_| scratch(".icc"))
        .transpose()?;
    if let (Some(file), Some(profile)) = (&mut icc, raster.color.icc_profile.as_deref()) {
        file.write_all(profile)?;
    }
    // PNG decoders may replace the sRGB rendering intent, and dwebp drops EXIF.
    // Supply a raw, fully oriented raster and explicit color encoding in these cases.
    let mut pam = None;
    let mut space = None;
    if orientation != 1 || icc.is_some() || raster.color.srgb.is_some() {
        let mut pixels = foundation::image_detection::open_image_with_limits(&raster.path)?;
        pixels.apply_orientation(
            image::metadata::Orientation::from_exif(orientation)
                .context("invalid source orientation")?,
        );
        if let Some(intent) = raster.color.srgb {
            let intent = match intent {
                png::SrgbRenderingIntent::Perceptual => "Per",
                png::SrgbRenderingIntent::RelativeColorimetric => "Rel",
                png::SrgbRenderingIntent::Saturation => "Sat",
                png::SrgbRenderingIntent::AbsoluteColorimetric => "Abs",
            };
            let model = if pixels.color().has_color() {
                "RGB_D65_SRG"
            } else {
                "Gra_D65"
            };
            space = Some(format!("{model}_{intent}_SRG"));
        }
        let mut file = scratch(".pam")?;
        pixels.write_to(&mut file, image::ImageFormat::Pnm)?;
        pam = Some(file);
    }
    let input = pam
        .as_ref()
        .map_or(raster.path.as_path(), NamedTempFile::path);
    let efforts = foundation::jxl_effort_policy::fast_lossless_pixel_efforts();
    let ((), _) = foundation::jxl_effort_policy::run_effort_search(
        source,
        staged_output,
        &efforts,
        |effort, output| {
            let mut encoder = foundation::CjxlBuilder::new();
            encoder
                .input(input)
                .output(output)
                .distance(0.0)
                .effort(effort)
                .threads(jpeg_encode_threads(options))
                .apple_compat(options.flags.contains(ConvertFlags::APPLE_COMPAT))
                .fixed_features(options.flags.contains(ConvertFlags::JXL_FIXED_FEATURES))
                .arg("--keep_invisible=1");
            if let Some(icc) = &icc {
                encoder.icc_profile(icc.path());
            } else if let Some(space) = &space {
                encoder.color_space(space);
            }
            let process = run_image_process(encoder.build())?;
            ensure!(
                process.status.success(),
                "cjxl d=0 effort {effort} failed: {}",
                String::from_utf8_lossy(&process.stderr).trim()
            );
            Ok(())
        },
        |()| true,
        |candidate| Ok(fs::metadata(candidate)?.len()),
    )?;
    raster.verify(source, staged_output)?;
    ensure!(
        foundation::common_utils::calculate_blake3_hash(source)? == before,
        "lossless source changed during encoding"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_file_tie_prefers_jxl() {
        assert!(fast_lossless_jxl_fits(42, 42));
        assert!(fast_lossless_jxl_fits(42, 41));
        assert!(!fast_lossless_jxl_fits(42, 43));
        assert!(!fast_lossless_jxl_fits(0, 0));
    }

    #[test]
    fn malformed_embedded_profile_is_not_assumed_srgb() -> Result<()> {
        let file = tempfile::NamedTempFile::new()?;
        let mut encoder = png::Encoder::new(file.reopen()?, 1, 1);
        encoder.set_color(png::ColorType::Rgb);
        let mut writer = encoder.write_header()?;
        writer.write_chunk(png::chunk::iCCP, b"profile\0\0invalid-deflate")?;
        writer.write_image_data(&[11, 22, 33])?;
        writer.finish()?;
        assert!(png_header(file.path()).is_err());
        Ok(())
    }

    #[test]
    #[ignore = "uses cjxl to create a genuine animated candidate"]
    fn matching_first_frame_does_not_prove_a_static_jxl() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let first = dir.path().join("still.png");
        image::RgbImage::from_pixel(1, 1, image::Rgb([1, 2, 3])).save(&first)?;
        let animation = dir.path().join("animation.png");
        let mut png = png::Encoder::new(File::create(&animation)?, 1, 1);
        png.set_color(png::ColorType::Rgb);
        png.set_animated(2, 0)?;
        let mut frames = png.write_header()?;
        frames.write_image_data(&[1, 2, 3])?;
        frames.write_image_data(&[4, 5, 6])?;
        frames.finish()?;
        let candidate = dir.path().join("animated.jxl");
        let encoded = run_image_process(
            foundation::CjxlBuilder::new()
                .input(animation)
                .output(&candidate)
                .distance(0.0)
                .effort(2)
                .build(),
        )?;
        ensure!(
            encoded.status.success(),
            "could not encode two-frame JXL fixture"
        );
        let err = verify_fast_lossless_jxl_pixels(&first, &candidate).unwrap_err();
        assert!(err.to_string().contains("complete single-image"));
        Ok(())
    }

    #[test]
    #[ignore = "uses cjxl, djxl, pngcheck and exiftool"]
    fn custom_synthetic_icc_survives_lossless_jxl() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let raw = dir.path().join("reference.ppm");
        let reference = dir.path().join("reference.jxl");
        let profile_path = dir.path().join("synthetic.icc");
        fs::write(&raw, b"P6\n1 1\n255\nRGB")?;
        let encoded = run_image_process(
            foundation::CjxlBuilder::new()
                .input(&raw)
                .output(&reference)
                .distance(0.0)
                .effort(2)
                .build(),
        )?;
        ensure!(
            encoded.status.success(),
            "synthetic ICC reference encode failed"
        );
        let mut decoder = foundation::DjxlBuilder::new()
            .input(&reference)
            .output(dir.path().join("reference.png"))
            .build();
        decoder.arg("--orig_icc_out").arg(&profile_path);
        ensure!(
            run_image_process(decoder)?.status.success(),
            "could not generate reference ICC"
        );
        let mut profile = fs::read(profile_path)?;
        ensure!(profile.len() >= 128, "missing synthetic ICC header");
        profile[48..52].copy_from_slice(b"MFB!"); // unique synthetic device manufacturer
        profile[84..100].fill(0); // optional profile ID
        let mut info = png::Info::with_size(1, 1);
        info.color_type = png::ColorType::Rgb;
        info.icc_profile = Some(profile.into());
        let source = dir.path().join("custom.png");
        let mut png = png::Encoder::with_info(File::create(&source)?, info)?.write_header()?;
        png.write_image_data(b"RGB")?;
        png.finish()?;
        let candidate = dir.path().join("custom.jxl");
        encode_fast_lossless_jxl(&source, &candidate, &ConvertOptions::default())?;
        verify_fast_lossless_jxl_pixels(&source, &candidate)?;
        // Exactly the same visible samples, but the baseline has another profile.
        assert!(verify_fast_lossless_jxl_pixels(&source, &reference).is_err());
        Ok(())
    }

    #[test]
    #[ignore = "uses cjxl, djxl, pngcheck and exiftool"]
    fn lossless_jxl_keeps_low_bits_hidden_rgb_and_srgb_intent() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let source = dir.path().join("rgba16-without-extension");
        let output = dir.path().join("candidate.jxl");
        let samples: Vec<u16> = (0..30_u16)
            .flat_map(|i| {
                [
                    0x1001 + i,
                    i,
                    0xff00 + i,
                    if i % 2 == 0 { 0 } else { 0xffff },
                ]
            })
            .collect();
        let write_fixture = |samples: &[u16], intent| -> Result<()> {
            let mut png = png::Encoder::new(File::create(&source)?, 6, 5);
            png.set_depth(png::BitDepth::Sixteen);
            png.set_color(png::ColorType::Rgba);
            png.set_source_srgb(intent);
            png.write_header()?.write_image_data(
                &samples
                    .iter()
                    .flat_map(|s| s.to_be_bytes())
                    .collect::<Vec<_>>(),
            )?;
            Ok(())
        };
        let intent = png::SrgbRenderingIntent::Perceptual;
        write_fixture(&samples, intent)?;
        let original = fs::read(&source)?;
        encode_fast_lossless_jxl(
            &source,
            &output,
            &ConvertOptions {
                child_threads: 1,
                ..Default::default()
            },
        )?;
        verify_fast_lossless_jxl_pixels(&source, &output)?;
        assert_eq!(fs::read(&source)?, original);
        write_fixture(&samples, png::SrgbRenderingIntent::RelativeColorimetric)?;
        assert!(verify_fast_lossless_jxl_pixels(&source, &output).is_err());
        let mut altered = samples;
        altered[0] ^= 1; // RGB below fully transparent alpha, a single 16-bit LSB.
        write_fixture(&altered, intent)?;
        assert!(verify_fast_lossless_jxl_pixels(&source, &output).is_err());
        Ok(())
    }
}
