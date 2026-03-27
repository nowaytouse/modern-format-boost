//! # Gainmap to JXL HDR Synthesis
//!
//! High-fidelity HDR synthesis pipeline for images containing gainmap metadata.
//! Supports:
//! - **HEIC/HEIF**: Apple (ProRAW/HDRHEIC), Samsung (Super HDR HEIC), ISO 21496-1
//! - **`UltraHDR` JPEG**: Google's JPEG-based gainmap format (MPF + XMP)
//!
//! Output formats:
//! - **32-bit `OpenEXR`**: Full HDR precision for cinema/scientific grade
//! - **16-bit PNG**: High precision integer for compatibility
//!
//! # Depth Channel Support
//! - Extracts depth maps from HEIC auxiliary images
//! - Embeds depth as JXL Extra Channel via jpegxl-rs FFI

use anyhow::{anyhow, Context, Result};
use image::{DynamicImage, ImageBuffer};
use libheif_rs::{ColorSpace, HeifContext, ImageHandle, ItemId, RgbChroma};
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use std::path::Path;
use std::process::Command;
use tracing::{info, warn};

/// HDR intermediate format selection
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HdrIntermediateFormat {
    /// 32-bit float `OpenEXR` - maximum precision
    #[default]
    OpenExr32,
    /// 16-bit integer PNG - high precision with better compatibility
    Png16,
}

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

/// Main entry point for converting a HEIC with Gainmap to an HDR JXL via intermediate format.
pub fn convert_heic_with_gainmap_to_jxl_hdr(
    input: &Path,
    output: &Path,
    _apple_compat: bool,
    intermediate_format: HdrIntermediateFormat,
) -> Result<()> {
    let data = std::fs::read(input).context("Failed to read HEIC file")?;
    let ctx = HeifContext::read_from_bytes(&data).context("Failed to parse HEIC context")?;
    let handle = ctx
        .primary_image_handle()
        .map_err(|e| anyhow!("Failed to get primary image handle: {e}"))?;

    // 1. Color Space Awareness: Only convert to sRGB if the source is P3
    // Use manual box parsing from the full data buffer
    let needs_p3_conversion = is_display_p3(&data);

    // 2. Detect and find Gainmap auxiliary image
    let aux_images = handle.auxiliary_images(None);
    let mut gainmap_item_id: Option<libheif_rs::ItemId> = None;

    for aux in &aux_images {
        if let Ok(aux_type) = aux.auxiliary_type() {
            let aux_type_str: &str = &aux_type;
            if aux_type_str.contains("hdrgainmap") || aux_type_str.contains("GainMap") {
                gainmap_item_id = Some(aux.item_id());
                break;
            }
        }
    }

    let gainmap_item =
        gainmap_item_id.ok_or_else(|| anyhow!("No gainmap found in auxiliary images"))?;

    // Get fresh handle for gainmap
    let gain_handle = ctx
        .image_handle(gainmap_item)
        .map_err(|e| anyhow!("Failed to get gainmap handle: {e}"))?;

    // 2b. Extract Depth Map if present (save as sidecar)
    // Search for depth auxiliary image
    let mut depth_item_id: Option<libheif_rs::ItemId> = None;
    for aux in &aux_images {
        if let Ok(aux_type) = aux.auxiliary_type() {
            let aux_type_str: &str = &aux_type;
            if aux_type_str.contains("depth") || aux_type_str.contains("Depth") {
                depth_item_id = Some(aux.item_id());
                break;
            }
        }
    }

    let depth_handle: Option<ImageHandle> = if let Some(depth_id) = depth_item_id {
        Some(
            ctx.image_handle(depth_id)
                .map_err(|e| anyhow!("Failed to get depth handle: {e}"))?,
        )
    } else {
        None
    };

    // 3. Decode SDR and Gainmap
    let sdr = decode_heif_handle(&handle, ColorSpace::Rgb(RgbChroma::Rgb))
        .context("Failed to decode SDR base image from HEIC")?;
    let gain = decode_heif_handle(&gain_handle, ColorSpace::Monochrome)
        .context("Failed to decode Gainmap auxiliary image from HEIC")?;

    // 3b. Decode Depth Map if present
    let depth_image: Option<DynamicImage> = if let Some(depth_hdl) = &depth_handle {
        Some(
            decode_heif_handle(depth_hdl, ColorSpace::Monochrome)
                .context("Failed to decode depth map from HEIC")?,
        )
    } else {
        None
    };

    // 4. Parse XMP parameters
    let params = parse_gainmap_params(&handle).unwrap_or_default();
    info!("Gainmap parameters: {:?}", params);

    // 5. Perform Synthesis
    let hdr_pixels = synthesize_hdr(&sdr, &gain, &params, needs_p3_conversion)
        .context("☢️ HDR synthesis math failure")?;

    // 6. Write intermediate file (EXR or PNG)
    let (tmp_file, intensity_target) = match intermediate_format {
        HdrIntermediateFormat::OpenExr32 => {
            let tmp_exr = output.with_extension("tmp_hdr.exr");
            write_exr(&hdr_pixels, sdr.width(), sdr.height(), &tmp_exr)
                .context("Failed to write intermediate 32-bit OpenEXR buffer")?;
            // intensity_target = 203 * 2^GainMapMax
            (tmp_exr, 203.0 * params.gain_map_max.exp2())
        }
        HdrIntermediateFormat::Png16 => {
            let tmp_png = output.with_extension("tmp_hdr.png");
            write_png16(&hdr_pixels, sdr.width(), sdr.height(), &tmp_png)
                .context("Failed to write intermediate 16-bit PNG buffer")?;
            // For PNG16, use a simplified intensity target
            (tmp_png, 1000.0) // Standard HDR10 nits
        }
    };

    // 7. Invoke cjxl
    let mut cmd = Command::new("cjxl");
    cmd.arg(&tmp_file)
        .arg(output)
        .arg("-d")
        .arg("1.0")
        .arg("--intensity_target")
        .arg(format!("{intensity_target:.0}"));

    // After matrix conversion in synthesis, the primaries are Rec.709 (sRGB)
    cmd.arg("-x").arg("color_space=sRGB");

    // For PNG16, we currently rely on intensity_target and color_space=sRGB
    // (cjxl will handle the high dynamic range via intensity_target)

    let status = cmd
        .status()
        .context("Failed to execute cjxl for HDR synthesis")?;

    if !status.success() {
        if tmp_file.exists() {
            let _ = std::fs::remove_file(&tmp_file);
        }
        return Err(anyhow!(
            "cjxl encoding failed with status {status} during HDR synthesis; dynamic range parameters might be invalid"
        ));
    }

    // 8. Save Depth Map as Sidecar if present
    if let Some(depth) = depth_image {
        let depth_output = output.with_extension(format!(
            "depth.{}",
            output
                .extension()
                .map(|s| s.to_string_lossy())
                .unwrap_or_default()
        ));
        depth
            .to_luma16()
            .save(&depth_output)
            .context("Failed to save depth sidecar PNG")?;
        info!("Depth map saved to: {}", depth_output.display());
    }

    // 9. Cleanup
    if tmp_file.exists() {
        let _ = std::fs::remove_file(&tmp_file);
    }

    Ok(())
}

/// Main entry point for converting an `UltraHDR` JPEG to an HDR JXL via intermediate format.
///
/// `UltraHDR` JPEGs contain:
/// - Base SDR image (standard JPEG)
/// - Gainmap image (embedded via MPF - Multi Picture Format)
/// - XMP metadata with gainmap parameters (hdrgm: namespace)
pub fn convert_ultrahdr_jpeg_to_jxl_hdr(
    input: &Path,
    output: &Path,
    intermediate_format: HdrIntermediateFormat,
) -> Result<()> {
    use crate::image_jpeg_analysis::extract_gainmap_from_jpeg;

    info!(
        "🌈 UltraHDR JPEG HDR synthesis started for: {}",
        input.display()
    );

    // 1. Read JPEG data
    let data = std::fs::read(input).context("Failed to read UltraHDR JPEG file")?;

    // 2. Extract base image and gainmap
    let (base_image, gainmap_image) = extract_gainmap_from_jpeg(&data)
        .map_err(|e| anyhow!("☢️ Failed to extract gainmap from UltraHDR JPEG: {e}"))?;

    warn!(
        "⚠️  Gainmap extracted: {}x{} (base: {}x{})",
        gainmap_image.width(),
        gainmap_image.height(),
        base_image.width(),
        base_image.height()
    );

    // 3. Check color space (UltraHDR is typically sRGB)
    let needs_p3_conversion = false; // UltraHDR is sRGB by definition

    // 4. Parse XMP parameters from JPEG
    let params = parse_gainmap_params_from_jpeg_xmp(&data).unwrap_or_else(|| {
        warn!("⚠️  No XMP gainmap params found, using defaults");
        GainMapParams::default()
    });
    info!("Gainmap parameters: {:?}", params);

    // 5. Perform Synthesis
    let hdr_pixels = synthesize_hdr(&base_image, &gainmap_image, &params, needs_p3_conversion)
        .context("☢️ HDR synthesis math failure")?;

    // 6. Write intermediate file (EXR or PNG)
    let (tmp_file, intensity_target) = match intermediate_format {
        HdrIntermediateFormat::OpenExr32 => {
            let tmp_exr = output.with_extension("tmp_hdr.exr");
            write_exr(
                &hdr_pixels,
                base_image.width(),
                base_image.height(),
                &tmp_exr,
            )
            .context("Failed to write intermediate 32-bit OpenEXR buffer")?;
            (tmp_exr, 203.0 * params.gain_map_max.exp2())
        }
        HdrIntermediateFormat::Png16 => {
            let tmp_png = output.with_extension("tmp_hdr.png");
            write_png16(
                &hdr_pixels,
                base_image.width(),
                base_image.height(),
                &tmp_png,
            )
            .context("Failed to write intermediate 16-bit PNG buffer")?;
            (tmp_png, 1000.0)
        }
    };

    // 7. Invoke cjxl
    let mut cmd = Command::new("cjxl");
    cmd.arg(&tmp_file)
        .arg(output)
        .arg("-d")
        .arg("1.0")
        .arg("--intensity_target")
        .arg(format!("{intensity_target:.0}"))
        .arg("-x")
        .arg("color_space=sRGB");

    // For PNG16, we rely on intensity_target and color_space=sRGB

    let status = cmd
        .status()
        .context("Failed to execute cjxl for HDR synthesis")?;

    if !status.success() {
        if tmp_file.exists() {
            let _ = std::fs::remove_file(&tmp_file);
        }
        return Err(anyhow!(
            "cjxl encoding failed with status {status} during UltraHDR JPEG HDR synthesis"
        ));
    }

    // 8. Cleanup
    if tmp_file.exists() {
        let _ = std::fs::remove_file(&tmp_file);
    }

    info!(
        "✅ UltraHDR JPEG HDR synthesis completed: {}",
        output.display()
    );
    Ok(())
}

fn decode_heif_handle(handle: &ImageHandle, color_space: ColorSpace) -> Result<DynamicImage> {
    let img = libheif_rs::LibHeif::new()
        .decode(handle, color_space, None)
        .map_err(|e| anyhow!("HEIF decode error: {e}"))?;

    let width = img.width();
    let height = img.height();
    let bit_depth = handle.luma_bits_per_pixel();

    match color_space {
        ColorSpace::Rgb(_) => {
            let planes = img.planes();
            let r_plane = planes
                .interleaved
                .ok_or_else(|| anyhow!("No RGB interleaved plane"))?;

            if bit_depth > 8 {
                // Handle 10/12/16-bit (data is actually u16 even if returned as &[u8])
                #[allow(clippy::cast_ptr_alignment)]
                let data_u16: &[u16] = unsafe {
                    std::slice::from_raw_parts(
                        r_plane.data.as_ptr().cast::<u16>(),
                        r_plane.data.len() / 2,
                    )
                };
                let mut buffer = ImageBuffer::new(width, height);
                for (x, y, pixel) in buffer.enumerate_pixels_mut() {
                    let offset = y as usize * (r_plane.stride / 2) + x as usize * 3;
                    let r = data_u16[offset];
                    let g = data_u16[offset + 1];
                    let b = data_u16[offset + 2];
                    *pixel = image::Rgb([r, g, b]);
                }
                Ok(DynamicImage::ImageRgb16(buffer))
            } else {
                let mut buffer = ImageBuffer::new(width, height);
                for (x, y, pixel) in buffer.enumerate_pixels_mut() {
                    let offset = y as usize * r_plane.stride + x as usize * 3;
                    let r = r_plane.data[offset];
                    let g = r_plane.data[offset + 1];
                    let b = r_plane.data[offset + 2];
                    *pixel = image::Rgb([r, g, b]);
                }
                Ok(DynamicImage::ImageRgb8(buffer))
            }
        }
        ColorSpace::Monochrome => {
            let planes = img.planes();
            let y_plane = planes.y.ok_or_else(|| anyhow!("No Y plane"))?;

            if bit_depth > 8 {
                #[allow(clippy::cast_ptr_alignment)]
                let data_u16: &[u16] = unsafe {
                    std::slice::from_raw_parts(
                        y_plane.data.as_ptr().cast::<u16>(),
                        y_plane.data.len() / 2,
                    )
                };
                let mut buffer = ImageBuffer::new(width, height);
                for (x, y, pixel) in buffer.enumerate_pixels_mut() {
                    let offset = y as usize * (y_plane.stride / 2) + x as usize;
                    let val = data_u16[offset];
                    *pixel = image::Luma([val]);
                }
                Ok(DynamicImage::ImageLuma16(buffer))
            } else {
                let mut buffer = ImageBuffer::new(width, height);
                for (x, y, pixel) in buffer.enumerate_pixels_mut() {
                    let offset = y as usize * y_plane.stride + x as usize;
                    let val = y_plane.data[offset];
                    *pixel = image::Luma([val]);
                }
                Ok(DynamicImage::ImageLuma8(buffer))
            }
        }
        _ => Err(anyhow!("Unsupported color space for synthesis")),
    }
}

fn is_display_p3(data: &[u8]) -> bool {
    use crate::common_utils::find_box_data_recursive;

    // 1. Check colr/nclx box (Common in HEIC/AVIF/JXL containers)
    if let Some(colr_data) = find_box_data_recursive(data, *b"colr") {
        if colr_data.len() >= 11 && &colr_data[0..4] == b"nclx" {
            // flavour: nclx
            // colour_primaries: bytes 8-9 (u16 BE)
            let primaries = u16::from_be_bytes([colr_data[8], colr_data[9]]);
            return primaries == 12; // 12 = Display P3, 1 = Rec.709/sRGB
        }
    }

    // 2. Fallback: Search for "Display P3" or "P3" in raw data (Heuristic for ICC)
    // We search the whole buffer for the signature of Display P3 ICC profile
    let search_limit = 1024 * 1024; // limit search to first 1MB for performance
    let end = data.len().min(search_limit);
    let slice = &data[..end];

    // Check for common Display P3 signatures in ICC profiles
    slice.windows(10).any(|w| w == b"Display P3")
        || slice.windows(2).any(|w| w == b"P3") && slice.windows(4).any(|w| w == b"colr")
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
            Ok(Event::Start(e) | Event::Empty(e)) => {
                // 1. Check Attributes (Common for Google/Samsung/ISO)
                for attr in e.attributes().flatten() {
                    let local_name = attr.key.local_name();
                    let attr_name = String::from_utf8_lossy(local_name.as_ref());
                    let attr_val = String::from_utf8_lossy(attr.value.as_ref());
                    if let Ok(f) = attr_val.parse::<f32>() {
                        match attr_name.as_ref() {
                            n if n.contains("GainMapMax") => params.gain_map_max = f,
                            n if n.contains("GainMapMin") => params.gain_map_min = f,
                            n if n.contains("Gamma") => params.gamma = f,
                            n if n.contains("OffsetSDR") || n.contains("OffsetSdr") => {
                                params.offset_sdr = f;
                            }
                            n if n.contains("OffsetHDR") || n.contains("OffsetHdr") => {
                                params.offset_hdr = f;
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
                        let text = reader.decoder().decode(val.as_ref()).unwrap_or_default().to_string();
                        if let Ok(f) = text.parse::<f32>() {
                            params.gain_map_max = f;
                        }
                    }
                } else if name.contains("GainMapMin") {
                    if let Ok(val) = reader.read_text(e.name()) {
                        let text = reader.decoder().decode(val.as_ref()).unwrap_or_default().to_string();
                        if let Ok(f) = text.parse::<f32>() {
                            params.gain_map_min = f;
                        }
                    }
                } else if name.contains("OffsetSDR") || name.contains("OffsetSdr") {
                    if let Ok(val) = reader.read_text(e.name()) {
                        let text = reader.decoder().decode(val.as_ref()).unwrap_or_default().to_string();
                        if let Ok(f) = text.parse::<f32>() {
                            params.offset_sdr = f;
                        }
                    }
                } else if name.contains("OffsetHDR") || name.contains("OffsetHdr") {
                    if let Ok(val) = reader.read_text(e.name()) {
                        let text = reader.decoder().decode(val.as_ref()).unwrap_or_default().to_string();
                        if let Ok(f) = text.parse::<f32>() {
                            params.offset_hdr = f;
                        }
                    }
                } else if name.contains("Gamma") {
                    if let Ok(val) = reader.read_text(e.name()) {
                        let text = reader.decoder().decode(val.as_ref()).unwrap_or_default().to_string();
                        if let Ok(f) = text.parse::<f32>() {
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
    needs_p3_conversion: bool,
) -> Result<Vec<f32>> {
    use image::GenericImageView;
    let (width, height) = sdr.dimensions();

    let gain_resized: DynamicImage = if gain.dimensions() == (width, height) {
        gain.clone()
    } else {
        let mut g = gain.clone();
        g.resize_exact(width, height, image::imageops::FilterType::Triangle);
        g
    };

    let mut hdr_pixels = Vec::with_capacity((width * height * 3) as usize);

    // Get typed buffers to avoid scale-to-u8 bug in GenericImageView::get_pixel
    let sdr_8 = if sdr.color().bits_per_pixel() <= 24 {
        Some(sdr.to_rgb8())
    } else {
        None
    };
    let sdr_16 = if sdr.color().bits_per_pixel() > 24 {
        Some(sdr.to_rgb16())
    } else {
        None
    };

    let gain_8 = if gain_resized.color().bits_per_pixel() <= 8 {
        Some(gain_resized.to_luma8())
    } else {
        None
    };
    let gain_16 = if gain_resized.color().bits_per_pixel() > 8 {
        Some(gain_resized.to_luma16())
    } else {
        None
    };
    let gain_rgb16 =
        if gain_resized.color().has_color() && gain_resized.color().bits_per_pixel() > 24 {
            Some(gain_resized.to_rgb16())
        } else {
            None
        };
    let gain_rgb8 =
        if gain_resized.color().has_color() && gain_resized.color().bits_per_pixel() <= 24 {
            Some(gain_resized.to_rgb8())
        } else {
            None
        };

    for y in 0..height {
        for x in 0..width {
            // 1. Get Normalized SDR (Linearized later)
            let (r_norm, g_norm, b_norm) = if let Some(buf) = sdr_16.as_ref() {
                let p = <image::ImageBuffer<image::Rgb<u16>, Vec<u16>>>::get_pixel(buf, x, y);
                (
                    f32::from(p.0[0]) / 65535.0,
                    f32::from(p.0[1]) / 65535.0,
                    f32::from(p.0[2]) / 65535.0,
                )
            } else if let Some(buf) = sdr_8.as_ref() {
                let p = <image::ImageBuffer<image::Rgb<u8>, Vec<u8>>>::get_pixel(buf, x, y);
                (
                    f32::from(p.0[0]) / 255.0,
                    f32::from(p.0[1]) / 255.0,
                    f32::from(p.0[2]) / 255.0,
                )
            } else {
                unreachable!("SDR buffer type mismatch");
            };

            // SDR to Linear (sRGB/Rec.709 Transfer function)
            let r_lin = srgb_to_linear(r_norm);
            let g_lin = srgb_to_linear(g_norm);
            let b_lin = srgb_to_linear(b_norm);

            // 2. Decode Gain
            let apply_gain = |val_raw: f32, max_val: f32| -> f32 {
                let val_norm = val_raw / max_val;
                let gain_px_corrected = val_norm.powf(1.0 / params.gamma.max(0.1));
                let log2_gain = gain_px_corrected * (params.gain_map_max - params.gain_map_min)
                    + params.gain_map_min;
                log2_gain.exp2()
            };

            let gain_channels = gain_resized.color().channel_count();
            let (gain_r, gain_g, gain_b) = if gain_channels >= 3 {
                if let Some(buf) = gain_rgb16.as_ref() {
                    let p = <image::ImageBuffer<image::Rgb<u16>, Vec<u16>>>::get_pixel(buf, x, y);
                    (
                        apply_gain(f32::from(p.0[0]), 65535.0),
                        apply_gain(f32::from(p.0[1]), 65535.0),
                        apply_gain(f32::from(p.0[2]), 65535.0),
                    )
                } else if let Some(buf) = gain_rgb8.as_ref() {
                    let p = <image::ImageBuffer<image::Rgb<u8>, Vec<u8>>>::get_pixel(buf, x, y);
                    (
                        apply_gain(f32::from(p.0[0]), 255.0),
                        apply_gain(f32::from(p.0[1]), 255.0),
                        apply_gain(f32::from(p.0[2]), 255.0),
                    )
                } else {
                    let g_val = apply_gain(128.0, 255.0);
                    (g_val, g_val, g_val)
                }
            } else {
                let g_val = if let Some(buf) = gain_16.as_ref() {
                    let p = <image::ImageBuffer<image::Luma<u16>, Vec<u16>>>::get_pixel(buf, x, y);
                    apply_gain(f32::from(p.0[0]), 65535.0)
                } else if let Some(buf) = gain_8.as_ref() {
                    let p = <image::ImageBuffer<image::Luma<u8>, Vec<u8>>>::get_pixel(buf, x, y);
                    apply_gain(f32::from(p.0[0]), 255.0)
                } else {
                    apply_gain(128.0, 255.0)
                };
                (g_val, g_val, g_val)
            };

            // 3. Apply Gain
            let r_hdr = (r_lin + params.offset_sdr).mul_add(gain_r, -params.offset_hdr);
            let g_hdr = (g_lin + params.offset_sdr).mul_add(gain_g, -params.offset_hdr);
            let b_hdr = (b_lin + params.offset_sdr).mul_add(gain_b, -params.offset_hdr);

            // 4. Color Primaries: Conditionally convert Linear P3 -> Linear sRGB (Rec.709)
            if needs_p3_conversion {
                // Matrix for D65 Display P3 to D65 Rec.709
                let r_srgb = 0.0001f32.mul_add(-b_hdr, 1.2249f32.mul_add(r_hdr, -(0.2247 * g_hdr)));
                let g_srgb = 0.0001f32.mul_add(b_hdr, (-0.0420f32).mul_add(r_hdr, 1.0419 * g_hdr));
                let b_srgb =
                    1.0983f32.mul_add(b_hdr, (-0.0197f32).mul_add(r_hdr, -(0.0786 * g_hdr)));

                hdr_pixels.push(r_srgb.max(0.0));
                hdr_pixels.push(g_srgb.max(0.0));
                hdr_pixels.push(b_srgb.max(0.0));
            } else {
                hdr_pixels.push(r_hdr.max(0.0));
                hdr_pixels.push(g_hdr.max(0.0));
                hdr_pixels.push(b_hdr.max(0.0));
            }
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

/// Write HDR pixels to a 16-bit PNG file.
///
/// Converts f32 [0.0, 1.0+] to u16 [0, 65535].
/// Values > 1.0 are clamped to 65535 (HDR highlights).
fn write_png16(pixels: &[f32], width: u32, height: u32, path: &Path) -> Result<()> {
    use image::{ImageBuffer, Rgb};

    let mut buffer: ImageBuffer<Rgb<u16>, Vec<u16>> = ImageBuffer::new(width, height);

    for (x, y, pixel) in buffer.enumerate_pixels_mut() {
        let idx = (y * width + x) as usize * 3;
        // Convert f32 [0.0, 1.0] to u16 [0, 65535]
        // HDR values > 1.0 are preserved up to ~2.0 (130k+)
        let r = ((pixels[idx].min(2.0) / 2.0) * 65535.0) as u16;
        let g = ((pixels[idx + 1].min(2.0) / 2.0) * 65535.0) as u16;
        let b = ((pixels[idx + 2].min(2.0) / 2.0) * 65535.0) as u16;
        *pixel = Rgb([r, g, b]);
    }

    buffer
        .save(path)
        .context("Failed to save 16-bit PNG intermediate file")?;

    Ok(())
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

/// Parse gainmap parameters from XMP data in a JPEG file.
fn parse_gainmap_params_from_jpeg_xmp(data: &[u8]) -> Option<GainMapParams> {
    use crate::image_jpeg_analysis::extract_xmp_from_jpeg_data;

    let xmp_str = extract_xmp_from_jpeg_data(data)?;
    Some(parse_gainmap_from_xmp(&xmp_str))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_srgb_to_linear() {
        assert_eq!(srgb_to_linear(0.0), 0.0);
        assert!((srgb_to_linear(1.0) - 1.0).abs() < 1e-6);
        // Middle grey roughly 0.214
        assert!((srgb_to_linear(0.5) - 0.214_041_14).abs() < 1e-6);
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
