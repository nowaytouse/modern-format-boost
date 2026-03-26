//! # HEIC Gainmap to JXL HDR Synthesis
//!
//! High-fidelity HDR synthesis pipeline for images containing gainmap metadata.
//! Primarily targeted at HEIC/HEIF files produced by Apple (ProRAW/HDRHEIC),
//! Samsung (Super HDR HEIC), and ISO 21496-1 compliant cameras.
//!
//! Note: Google's "Ultra HDR" branding refers to a JPEG-based container
//! format with embedded gainmaps. While this module detects standard gainmap
//! markers, the synthesis path is currently optimized for HEIC auxiliary image streams.

use anyhow::{anyhow, Context, Result};
use image::{DynamicImage, GenericImageView, ImageBuffer, Rgb};
use libheif_rs::{ColorSpace, HeifContext, ImageHandle, ItemId, RgbChroma};
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use std::path::Path;
use std::process::Command;
use tracing::info;

#[derive(Debug, Clone, Copy)]
pub struct GainMapParams {
    pub gain_map_max: f32,
    pub gain_map_min: f32,
    pub gamma: f32,
    pub offset_sdr: f32,
    pub offset_hdr: f32,
}

impl Default for GainMapParams {
    fn default() -> Self {
        Self {
            gain_map_max: 1.0, // 2x gain
            gain_map_min: 0.0,
            gamma: 1.0,
            offset_sdr: 1.0 / 64.0,
            offset_hdr: 1.0 / 64.0,
        }
    }
}

/// Main entry point for converting a HEIC with Gainmap to an HDR JXL via intermediate EXR.
pub fn convert_heic_with_gainmap_to_jxl_hdr(
    input: &Path,
    output: &Path,
    apple_compat: bool,
) -> Result<()> {
    let data = std::fs::read(input).context("Failed to read HEIC file")?;
    let ctx = HeifContext::read_from_bytes(&data).context("Failed to parse HEIC context")?;
    let primary_handle = ctx
        .primary_image_handle()
        .context("No primary image in HEIC")?;

    // 1. Detect and find Gainmap auxiliary image
    let aux_images = primary_handle.auxiliary_images(None);
    let mut gainmap_handle: Option<ImageHandle> = None;

    for aux in aux_images {
        if let Ok(aux_type) = aux.auxiliary_type() {
            let aux_type_str: &str = &aux_type;
            if aux_type_str.contains("hdrgainmap") || aux_type_str.contains("GainMap") {
                gainmap_handle = Some(aux);
                break;
            }
        }
    }

    let gain_handle =
        gainmap_handle.ok_or_else(|| anyhow!("No gainmap found in auxiliary images"))?;

    // 2. Decode SDR and Gainmap
    let sdr_img = decode_heif_handle(&primary_handle, ColorSpace::Rgb(RgbChroma::Rgb))
        .context("Failed to decode SDR base image from HEIC")?;
    let gain_img = decode_heif_handle(&gain_handle, ColorSpace::Monochrome)
        .context("Failed to decode Gainmap auxiliary image from HEIC")?;

    // 3. Parse XMP parameters
    let params = parse_gainmap_params(&primary_handle).unwrap_or_default();
    info!("Gainmap parameters: {:?}", params);

    // 4. Perform Synthesis
    let hdr_pixels = synthesize_hdr(&sdr_img, &gain_img, &params)
        .context("HDR synthesis math failed (linear light mapping)")?;

    // 5. Write intermediate EXR
    let tmp_exr = output.with_extension("tmp_hdr.exr");
    write_exr(&hdr_pixels, sdr_img.width(), sdr_img.height(), &tmp_exr)
        .context("Failed to write intermediate 32-bit OpenEXR buffer")?;

    // 6. Invoke cjxl
    // intensity_target = 203 * 2^GainMapMax
    let intensity_target = 203.0 * 2.0_f32.powf(params.gain_map_max);

    let mut cmd = Command::new("cjxl");
    cmd.arg(&tmp_exr)
        .arg(output)
        .arg("-d")
        .arg("1.0")
        .arg("--intensity_target")
        .arg(format!("{:.0}", intensity_target))
        .arg("--transfer_function")
        .arg("linear");

    if apple_compat {
        cmd.arg("--color_space").arg("RGB_D65_P3_Rel_Lin");
    } else {
        cmd.arg("--color_space").arg("RGB_D65_SRG_Rel_Lin");
    }

    let status = cmd
        .status()
        .context("Failed to execute cjxl for HDR synthesis")?;

    if !status.success() {
        if tmp_exr.exists() {
            let _ = std::fs::remove_file(&tmp_exr);
        }
        return Err(anyhow!(
            "cjxl encoding failed with status {} during HDR synthesis; dynamic range parameters might be invalid",
            status
        ));
    }

    // 7. Cleanup
    if tmp_exr.exists() {
        let _ = std::fs::remove_file(&tmp_exr);
    }

    Ok(())
}

fn decode_heif_handle(handle: &ImageHandle, color_space: ColorSpace) -> Result<DynamicImage> {
    let img = libheif_rs::LibHeif::new()
        .decode(handle, color_space, None)
        .map_err(|e| anyhow!("HEIF decode error: {}", e))?;

    let width = img.width();
    let height = img.height();

    match color_space {
        ColorSpace::Rgb(_) => {
            let planes = img.planes();
            let r_plane = planes
                .interleaved
                .ok_or_else(|| anyhow!("No RGB interleaved plane"))?;
            let mut buffer = ImageBuffer::new(width, height);
            for (x, y, pixel) in buffer.enumerate_pixels_mut() {
                let offset = y as usize * r_plane.stride + x as usize * 3;
                let r = r_plane.data[offset];
                let g = r_plane.data[offset + 1];
                let b = r_plane.data[offset + 2];
                *pixel = Rgb([r, g, b]);
            }
            Ok(DynamicImage::ImageRgb8(buffer))
        }
        ColorSpace::Monochrome => {
            let planes = img.planes();
            let y_plane = planes.y.ok_or_else(|| anyhow!("No Y plane"))?;
            let mut buffer = ImageBuffer::new(width, height);
            for (x, y, pixel) in buffer.enumerate_pixels_mut() {
                let offset = y as usize * y_plane.stride + x as usize;
                let val = y_plane.data[offset];
                *pixel = image::Luma([val]);
            }
            Ok(DynamicImage::ImageLuma8(buffer))
        }
        _ => Err(anyhow!("Unsupported color space for synthesis")),
    }
}

fn parse_gainmap_params(handle: &ImageHandle) -> Option<GainMapParams> {
    let mut ids = [ItemId::default(); 1];
    let count = handle.metadata_block_ids(&mut ids, b"xmp ");
    if count == 0 {
        return None;
    }

    let xmp_data = handle.metadata(ids[0]).ok()?;
    let xmp_str = String::from_utf8_lossy(&xmp_data);
    Some(parse_gainmap_from_xmp(&xmp_str))
}

fn parse_gainmap_from_xmp(xmp_str: &str) -> GainMapParams {
    let mut params = GainMapParams::default();
    let mut reader = Reader::from_str(xmp_str);
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                // 1. Check Attributes (Common for Google/Samsung/ISO)
                for attr in e.attributes().flatten() {
                    let local_name = attr.key.local_name();
                    let attr_name = String::from_utf8_lossy(local_name.as_ref());
                    let attr_val = String::from_utf8_lossy(&attr.value);
                    if let Ok(f) = attr_val.parse::<f32>() {
                        match attr_name.as_ref() {
                            n if n.contains("GainMapMax") => params.gain_map_max = f,
                            n if n.contains("GainMapMin") => params.gain_map_min = f,
                            n if n.contains("Gamma") => params.gamma = f,
                            n if n.contains("OffsetSDR") || n.contains("OffsetSdr") => {
                                params.offset_sdr = f
                            }
                            n if n.contains("OffsetHDR") || n.contains("OffsetHdr") => {
                                params.offset_hdr = f
                            }
                            _ => (),
                        }
                    }
                }

                // 2. Check Child Elements (Common for Apple)
                let name_bytes = e.name();
                let name = String::from_utf8_lossy(name_bytes.as_ref());

                if name.contains("GainMapMax") {
                    if let Ok(val) = reader.read_text(e.name()) {
                        if let Ok(f) = val.parse::<f32>() {
                            params.gain_map_max = f;
                        }
                    }
                } else if name.contains("GainMapMin") {
                    if let Ok(val) = reader.read_text(e.name()) {
                        if let Ok(f) = val.parse::<f32>() {
                            params.gain_map_min = f;
                        }
                    }
                } else if name.contains("OffsetSDR") || name.contains("OffsetSdr") {
                    if let Ok(val) = reader.read_text(e.name()) {
                        if let Ok(f) = val.parse::<f32>() {
                            params.offset_sdr = f;
                        }
                    }
                } else if name.contains("OffsetHDR") || name.contains("OffsetHdr") {
                    if let Ok(val) = reader.read_text(e.name()) {
                        if let Ok(f) = val.parse::<f32>() {
                            params.offset_hdr = f;
                        }
                    }
                } else if name.contains("Gamma") {
                    if let Ok(val) = reader.read_text(e.name()) {
                        if let Ok(f) = val.parse::<f32>() {
                            params.gamma = f;
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            _ => (),
        }
        buf.clear();
    }
    params
}

fn synthesize_hdr(
    sdr: &DynamicImage,
    gain: &DynamicImage,
    params: &GainMapParams,
) -> Result<Vec<f32>> {
    let (width, height) = sdr.dimensions();

    let gain_resized: DynamicImage = if gain.dimensions() != (width, height) {
        gain.resize_exact(width, height, image::imageops::FilterType::Triangle)
    } else {
        gain.clone()
    };

    let sdr_rgb = sdr.to_rgb8();
    let gain_luma = gain_resized.to_luma8();
    let mut hdr_pixels = Vec::with_capacity((width * height * 3) as usize);

    for y in 0..height {
        for x in 0..width {
            let sdr_px = sdr_rgb.get_pixel(x, y);
            let gain_px = gain_luma.get_pixel(x, y).0[0] as f32 / 255.0;

            // 1. SDR to Linear
            let r_lin = srgb_to_linear(sdr_px[0] as f32 / 255.0);
            let g_lin = srgb_to_linear(sdr_px[1] as f32 / 255.0);
            let b_lin = srgb_to_linear(sdr_px[2] as f32 / 255.0);

            // 2. Decode Gain
            let log2_gain =
                gain_px * (params.gain_map_max - params.gain_map_min) + params.gain_map_min;
            let gain_val = 2.0_f32.powf(log2_gain);

            // 3. Apply Gain
            let r_hdr = (r_lin + params.offset_sdr) * gain_val - params.offset_hdr;
            let g_hdr = (g_lin + params.offset_sdr) * gain_val - params.offset_hdr;
            let b_hdr = (b_lin + params.offset_sdr) * gain_val - params.offset_hdr;

            hdr_pixels.push(r_hdr.max(0.0));
            hdr_pixels.push(g_hdr.max(0.0));
            hdr_pixels.push(b_hdr.max(0.0));
        }
    }

    Ok(hdr_pixels)
}

fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

fn write_exr(pixels: &[f32], width: u32, height: u32, path: &Path) -> Result<()> {
    use exr::prelude::*;

    write_rgb_file(path, width as usize, height as usize, |x, y| {
        let idx = (y * width as usize + x) * 3;
        (pixels[idx], pixels[idx + 1], pixels[idx + 2])
    })
    .context("Failed to write EXR file")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_srgb_to_linear() {
        assert_eq!(srgb_to_linear(0.0), 0.0);
        assert!((srgb_to_linear(1.0) - 1.0).abs() < 1e-6);
        // Middle grey roughly 0.214
        assert!((srgb_to_linear(0.5) - 0.21404114).abs() < 1e-6);
    }

    #[test]
    fn test_parse_gainmap_apple() {
        let xmp = r#"
            <xmpmeta xmlns:x="adobe:ns:meta/">
                <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
                    <rdf:Description rdf:about="" xmlns:apple-aux="http://ns.apple.com/apple-aux/1.0/">
                        <apple-aux:GainMapMax>3.0</apple-aux:GainMapMax>
                        <apple-aux:GainMapMin>1.0</apple-aux:GainMapMin>
                        <apple-aux:OffsetSdr>0.01</apple-aux:OffsetSdr>
                        <apple-aux:OffsetHdr>0.02</apple-aux:OffsetHdr>
                    </rdf:Description>
                </rdf:RDF>
            </xmpmeta>
        "#;
        let params = parse_gainmap_from_xmp(xmp);
        assert_eq!(params.gain_map_max, 3.0);
        assert_eq!(params.gain_map_min, 1.0);
        assert_eq!(params.offset_sdr, 0.01);
        assert_eq!(params.offset_hdr, 0.02);
    }

    #[test]
    fn test_parse_gainmap_iso_samsung() {
        let xmp = r#"
            <xmpmeta xmlns:x="adobe:ns:meta/">
                <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
                    <rdf:Description rdf:about="" 
                        xmlns:hdrgm="http://ns.adobe.com/hdr-gain-map/1.0/"
                        hdrgm:GainMapMax="4.5"
                        hdrgm:GainMapMin="0.5"
                        hdrgm:Gamma="2.2"
                        hdrgm:OffsetSDR="0.05"
                        hdrgm:OffsetHDR="0.08" />
                </rdf:RDF>
            </xmpmeta>
        "#;
        let params = parse_gainmap_from_xmp(xmp);
        assert_eq!(params.gain_map_max, 4.5);
        assert_eq!(params.gain_map_min, 0.5);
        assert_eq!(params.gamma, 2.2);
        assert_eq!(params.offset_sdr, 0.05);
        assert_eq!(params.offset_hdr, 0.08);
    }
}
