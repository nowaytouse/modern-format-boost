//! # HDR and Color Space unification
//!
//! Consolidated module for HDR decoding, synthesis, and color space utilities.

use anyhow::{Context, Result, anyhow};
use image::{DynamicImage, ImageBuffer};
use libheif_rs::{ColorSpace, HeifContext, ImageHandle, ItemId, RgbChroma};
use quick_xml::XmlVersion;
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use serde::{Deserialize, Serialize};
use std::env;
use std::path::{Path, PathBuf};

use crate::builder_base::ToolBuilder;
use crate::ffprobe_json::ColorInfo;
use crate::image_builders::ExiftoolBuilder;
use crate::image_jpeg_analysis::extract_gainmap_from_jpeg;
use crate::jxl_builder::CjxlBuilder;
use crate::unified_error::ImgQualityError;

fn read_native_u16_word(data: &[u8], word_index: usize) -> Option<u16> {
    let byte_index = word_index.checked_mul(2)?;
    let bytes = data.get(byte_index..byte_index + 2)?;
    Some(u16::from_ne_bytes([bytes[0], bytes[1]]))
}

/// HDR intermediate format selection
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HdrIntermediateFormat {
    /// 32-bit float `OpenEXR` - maximum precision
    #[default]
    OpenExr32,
    /// 16-bit integer PNG - high precision with better compatibility
    Png16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "fuzzing", derive(arbitrary::Arbitrary))]
pub struct GainMapParams {
    pub gain_map_max: f32,
    pub gain_map_min: f32,
    pub gamma: f32,
    pub offset_sdr: f32,
    pub offset_hdr: f32,
    /// ISO 21496-1: Scaling for the gain map range.
    pub use_base_color_space: bool,
    /// ISO 21496-1: Indicates if the base rendition is HDR.
    pub base_rendition_is_hdr: bool,
}

impl Default for GainMapParams {
    fn default() -> Self {
        Self {
            gain_map_max: 1.0, // 2x gain
            gain_map_min: 0.0,
            gamma: 1.0,
            offset_sdr: crate::constants::HDR_GAINMAP_OFFSET_SDR,
            offset_hdr: crate::constants::HDR_GAINMAP_OFFSET_HDR,
            use_base_color_space: true,
            base_rendition_is_hdr: false,
        }
    }
}

/// Main entry point for converting a `HEIC` with Gainmap to an HDR `JXL` via intermediate format.
/// # Errors
/// Returns an error if the conversion fails due to invalid input or processing errors.
#[allow(
    clippy::too_many_lines,
    reason = "Complex HDR conversion logic requiring multiple steps and error handling"
)]
pub fn convert_heic_with_gainmap_to_jxl_hdr(
    input: &Path,
    output: &Path,
    apple_compat: bool,
    intermediate_format: HdrIntermediateFormat,
    ultimate: bool,
) -> Result<()> {
    let actual_distance = crate::constants::jxl_distance_for_mode(1.0, ultimate);
    let actual_effort = crate::constants::jxl_effort_for_mode(ultimate);

    let file_label = input
        .file_name()
        .map_or_else(|| "unknown_heic".into(), |s| s.to_string_lossy());

    log_detail!(&format!("Starting HEIC to HDR JXL synthesis: {file_label}"));

    let data = std::fs::read(input).context("Failed to read HEIC file")?;
    let ctx = HeifContext::read_from_bytes(&data).context("Failed to parse HEIC context")?;
    let handle = ctx
        .primary_image_handle()
        .map_err(|e| anyhow!("Failed to get primary image handle: {e}"))?;

    let needs_p3_conversion = is_display_p3(&data);

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

    let gain_handle = ctx
        .image_handle(gainmap_item)
        .map_err(|e| anyhow!("Failed to get gainmap handle: {e}"))?;

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

    let sdr = decode_heif_handle(&handle, ColorSpace::Rgb(RgbChroma::Rgb))
        .context("Failed to decode SDR base image from HEIC")?;
    let gain = decode_heif_handle(&gain_handle, ColorSpace::Monochrome)
        .context("Failed to decode Gainmap auxiliary image from HEIC")?;

    let depth_image: Option<DynamicImage> = if let Some(depth_hdl) = &depth_handle {
        Some(
            decode_heif_handle(depth_hdl, ColorSpace::Monochrome)
                .context("Failed to decode depth map from HEIC")?,
        )
    } else {
        None
    };

    let params = parse_gainmap_params(&handle)
        .ok_or_else(|| anyhow!("Failed to parse gainmap parameters from HEIC XMP metadata"))?;

    log_detail!(&format!(
        "Performing HDR GainMap synthesis for {file_label} (P3={needs_p3_conversion})"
    ));

    let hdr_pixels = synthesize_hdr(&sdr, &gain, &params, needs_p3_conversion)
        .context("☢️ HDR synthesis math failure")?;

    let (tmp_file, intensity_target) = match intermediate_format {
        HdrIntermediateFormat::OpenExr32 => {
            let tmp_exr = output.with_extension("tmp_hdr.exr");
            write_exr(&hdr_pixels, sdr.width(), sdr.height(), &tmp_exr)
                .context("Failed to write intermediate 32-bit OpenEXR buffer")?;
            (
                tmp_exr,
                f64::from(crate::constants::HDR_REFERENCE_WHITE_NITS)
                    * f64::from(params.gain_map_max.exp2()),
            )
        }
        HdrIntermediateFormat::Png16 => {
            let tmp_png = output.with_extension("tmp_hdr.png");
            write_png16(&hdr_pixels, sdr.width(), sdr.height(), &tmp_png)
                .context("Failed to write intermediate 16-bit PNG buffer")?;
            (
                tmp_png,
                f64::from(crate::constants::HDR_REFERENCE_WHITE_NITS)
                    * f64::from(params.gain_map_max.exp2()),
            )
        }
    };

    let mut builder = crate::CjxlBuilder::new();
    builder
        .input(&tmp_file)
        .output(output)
        .distance(actual_distance)
        .effort(actual_effort)
        .apple_compat(apple_compat)
        .arg("-x")
        .arg("color_space=RGB_D65_SRG_Rel_PeQ");

    if let Some(target_f32) =
        crate::numeric_cast::f64_to_f32_strict(intensity_target, "intensity_target_raw")
    {
        if let Some(it) = resolve_intensity_target(target_f32) {
            builder.intensity_target(crate::numeric_cast::f64_to_f32_lossy(f64::from(it)));
            log_info!(
                crate::static_logs::messages::LABEL_CALIBRATION,
                &format!("Applying intensity_target {it} for HDR synthesis")
            );
        } else {
            log_anomaly!(
                crate::static_logs::messages::LABEL_CALIBRATION,
                "No valid intensity_target — proceeding without --intensity_target"
            );
        }
    } else {
        log_anomaly!(
            crate::static_logs::messages::LABEL_CALIBRATION,
            "Invalid intensity_target float conversion — proceeding without --intensity_target"
        );
    }

    let status = builder
        .build()
        .status()
        .context("Failed to execute cjxl for HDR synthesis")?;

    if !status.success() {
        if tmp_file.exists() {
            let _ = std::fs::remove_file(&tmp_file);
        }
        return Err(anyhow!(
            "cjxl encoding failed with status {status} during HDR synthesis"
        ));
    }

    if let Some(depth) = depth_image {
        let ext = output
            .extension()
            .map_or_else(|| "jxl".into(), |s| s.to_string_lossy());
        let depth_output = output.with_extension(format!("depth.{ext}"));
        depth
            .to_luma16()
            .save(&depth_output)
            .context("Failed to save depth sidecar PNG")?;
        log_info!(
            crate::static_logs::messages::LABEL_DETECTION,
            &format!("Depth map saved to: {}", depth_output.display())
        );
    }

    if tmp_file.exists() {
        let _ = std::fs::remove_file(&tmp_file);
    }

    Ok(())
}

/// # Errors
/// Returns an error if the conversion fails due to invalid input or processing errors.
#[allow(
    clippy::too_many_lines,
    reason = "Complex HDR conversion logic requiring multiple steps and error handling"
)]
pub fn convert_ultrahdr_jpeg_to_jxl_hdr(
    input: &Path,
    output: &Path,
    apple_compat: bool,
    intermediate_format: HdrIntermediateFormat,
    ultimate: bool,
) -> Result<()> {
    let actual_distance = crate::constants::jxl_distance_for_mode(1.0, ultimate);
    let actual_effort = crate::constants::jxl_effort_for_mode(ultimate);

    log_info!(
        crate::static_logs::messages::LABEL_CONVERSION,
        &format!(
            "UltraHDR JPEG HDR synthesis started for: {}",
            input.display()
        )
    );

    let data = std::fs::read(input).context("Failed to read UltraHDR JPEG file")?;

    let (base_image, gainmap_image) = extract_gainmap_from_jpeg(&data)
        .map_err(|e| anyhow!("☢️ Failed to extract gainmap from UltraHDR JPEG: {e}"))?;

    log_detail!(&format!(
        "Gainmap extracted: {}x{} (base: {}x{})",
        gainmap_image.width(),
        gainmap_image.height(),
        base_image.width(),
        base_image.height()
    ));

    let needs_p3_conversion = false;

    let params = parse_gainmap_params_from_jpeg_xmp(&data)
        .ok_or_else(|| anyhow::anyhow!("No valid XMP gainmap parameters found in the image"))?;

    log_detail!(&format!("Gainmap parameters: {params:?}"));

    let hdr_pixels = synthesize_hdr(&base_image, &gainmap_image, &params, needs_p3_conversion)
        .context("☢️ HDR synthesis math failure")?;

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
            (
                tmp_exr,
                f64::from(crate::constants::HDR_REFERENCE_WHITE_NITS)
                    * f64::from(params.gain_map_max.exp2()),
            )
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
            (
                tmp_png,
                f64::from(crate::constants::HDR_REFERENCE_WHITE_NITS)
                    * f64::from(params.gain_map_max.exp2()),
            )
        }
    };

    let mut builder = crate::CjxlBuilder::new();
    builder
        .input(&tmp_file)
        .output(output)
        .distance(actual_distance)
        .effort(actual_effort)
        .apple_compat(apple_compat)
        .arg("-x")
        .arg("color_space=RGB_D65_SRG_Rel_PeQ");

    if let Some(target_f32) =
        crate::numeric_cast::f64_to_f32_strict(intensity_target, "intensity_target_raw")
    {
        if let Some(it) = resolve_intensity_target(target_f32) {
            builder.intensity_target(crate::numeric_cast::f64_to_f32_lossy(f64::from(it)));
            log_info!(
                crate::static_logs::messages::LABEL_CALIBRATION,
                &format!("Applying intensity_target {it} for UltraHDR synthesis")
            );
        } else {
            log_anomaly!(
                crate::static_logs::messages::LABEL_CALIBRATION,
                "No valid intensity_target — proceeding without --intensity_target"
            );
        }
    } else {
        log_anomaly!(
            crate::static_logs::messages::LABEL_CALIBRATION,
            "Invalid intensity_target float conversion — proceeding without --intensity_target"
        );
    }

    let status = builder
        .build()
        .status()
        .context("Failed to execute cjxl for HDR synthesis")?;

    if !status.success() {
        if tmp_file.exists() {
            let _ = std::fs::remove_file(&tmp_file);
        }
        return Err(anyhow!(
            "cjxl encoding failed during UltraHDR JPEG HDR synthesis"
        ));
    }

    if tmp_file.exists() {
        let _ = std::fs::remove_file(&tmp_file);
    }

    Ok(())
}

/// # Errors
/// Returns an error if the conversion fails due to invalid input or processing errors.
pub fn convert_ultrahdr_jpeg_to_jxl_migration(
    input: &Path,
    output: &Path,
    distance: f32,
    _effort: u8,
    apple_compat: bool,
    ultimate: bool,
) -> Result<()> {
    let actual_distance = crate::constants::jxl_distance_for_mode(distance, ultimate);
    let actual_effort = crate::constants::jxl_effort_for_mode(ultimate);

    log_info!(
        crate::static_logs::messages::LABEL_CONVERSION,
        &format!(
            "📤 UltraHDR JPEG Gainmap migration started: {}",
            input.display()
        )
    );

    let data = std::fs::read(input).context("Failed to read UltraHDR JPEG file")?;
    let (_base_image, gainmap_image) = extract_gainmap_from_jpeg(&data)
        .map_err(|e| anyhow!("Failed to extract gainmap for migration: {e}"))?;

    let gainmap_sidecar = output.with_extension("gainmap.png");
    gainmap_image
        .save(&gainmap_sidecar)
        .context("Failed to save gainmap sidecar file")?;

    log_info!(
        crate::static_logs::messages::LABEL_COPY,
        &format!("💾 Gainmap saved as sidecar: {}", gainmap_sidecar.display())
    );

    let encode_status = CjxlBuilder::new()
        .input(input)
        .output(output)
        .lossless_jpeg(true)
        .distance(actual_distance)
        .effort(actual_effort)
        .apple_compat(apple_compat)
        .build()
        .status()
        .context("Failed to spawn cjxl for UltraHDR migration")?;

    if !encode_status.success() {
        return Err(anyhow!(
            "Lossless JPEG recompression failed for UltraHDR migration"
        ));
    }

    let mut exiftool = ExiftoolBuilder::new();
    exiftool
        .tags_from_file(input)
        .input(output)
        .overwrite_original()
        .preserve_date()
        .ignore_minor();

    if let Err(e) = exiftool.build().status() {
        log_anomaly!(
            crate::static_logs::messages::LABEL_METADATA,
            &format!(
                "Metadata preservation warning for {}: {}",
                output.display(),
                e
            )
        );
    }

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
                let mut buffer = ImageBuffer::new(width, height);
                for (x, y, pixel) in buffer.enumerate_pixels_mut() {
                    let y_usize = usize::try_from(y)?;
                    let x_usize = usize::try_from(x)?;
                    let offset = y_usize
                        .checked_mul(r_plane.stride / 2)
                        .and_then(|v| v.checked_add(x_usize.checked_mul(3)?))
                        .ok_or_else(|| anyhow!("RGB16 offset calculation overflow"))?;
                    let r = read_native_u16_word(r_plane.data, offset)
                        .ok_or_else(|| anyhow!("RGB16 plane buffer shorter than expected"))?;
                    let g = read_native_u16_word(r_plane.data, offset + 1)
                        .ok_or_else(|| anyhow!("RGB16 plane buffer shorter than expected"))?;
                    let b = read_native_u16_word(r_plane.data, offset + 2)
                        .ok_or_else(|| anyhow!("RGB16 plane buffer shorter than expected"))?;
                    *pixel = image::Rgb([r, g, b]);
                }
                Ok(DynamicImage::ImageRgb16(buffer))
            } else {
                let mut buffer = ImageBuffer::new(width, height);
                for (x, y, pixel) in buffer.enumerate_pixels_mut() {
                    let y_usize = usize::try_from(y)?;
                    let x_usize = usize::try_from(x)?;
                    let offset = y_usize
                        .saturating_mul(r_plane.stride)
                        .saturating_add(x_usize.saturating_mul(3));
                    if offset + 2 < r_plane.data.len() {
                        let r = r_plane.data[offset];
                        let g = r_plane.data[offset + 1];
                        let b = r_plane.data[offset + 2];
                        *pixel = image::Rgb([r, g, b]);
                    }
                }
                Ok(DynamicImage::ImageRgb8(buffer))
            }
        }
        ColorSpace::Monochrome => {
            let planes = img.planes();
            let y_plane = planes.y.ok_or_else(|| anyhow!("No Y plane"))?;

            if bit_depth > 8 {
                let mut buffer = ImageBuffer::new(width, height);
                for (x, y, pixel) in buffer.enumerate_pixels_mut() {
                    let y_usize = usize::try_from(y)?;
                    let x_usize = usize::try_from(x)?;
                    let offset = y_usize
                        .checked_mul(y_plane.stride / 2)
                        .and_then(|v| v.checked_add(x_usize))
                        .ok_or_else(|| anyhow!("Luma16 offset calculation overflow"))?;
                    let val = read_native_u16_word(y_plane.data, offset)
                        .ok_or_else(|| anyhow!("Luma16 plane buffer shorter than expected"))?;
                    *pixel = image::Luma([val]);
                }
                Ok(DynamicImage::ImageLuma16(buffer))
            } else {
                let mut buffer = ImageBuffer::new(width, height);
                for (x, y, pixel) in buffer.enumerate_pixels_mut() {
                    let y_usize = usize::try_from(y)?;
                    let x_usize = usize::try_from(x)?;
                    let offset = y_usize
                        .saturating_mul(y_plane.stride)
                        .saturating_add(x_usize);
                    if offset < y_plane.data.len() {
                        let val = y_plane.data[offset];
                        *pixel = image::Luma([val]);
                    }
                }
                Ok(DynamicImage::ImageLuma8(buffer))
            }
        }
        _ => Err(anyhow!("Unsupported color space for synthesis")),
    }
}

fn is_display_p3(data: &[u8]) -> bool {
    if let Some(colr_data) = crate::common_utils::find_box_data_recursive(data, *b"colr")
        && colr_data.len() >= 11
        && colr_data.get(0..4) == Some(b"nclx")
    {
        let primaries = u16::from_be_bytes([colr_data[8], colr_data[9]]);
        return primaries == crate::constants::COLOR_PRIMARY_P3;
    }
    let search_limit = crate::constants::ICC_SEARCH_LIMIT_BYTES;
    let end = data.len().min(search_limit);
    let slice = data.get(..end).unwrap_or(&[]);
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
    parse_gainmap_from_xmp(&xmp_data)
}

fn parse_gainmap_from_xmp(xmp_data: &[u8]) -> Option<GainMapParams> {
    let mut params = GainMapParams::default();
    let mut reader = Reader::from_reader(xmp_data);
    let mut buf = Vec::new();
    let mut found_any = false;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e) | Event::Empty(e)) => {
                for attr in e.attributes().flatten() {
                    let local_name = attr.key.local_name();
                    if let Ok(attr_val_cow) = attr.normalized_value(XmlVersion::Explicit1_0)
                        && let Ok(f) = attr_val_cow.parse::<f32>()
                    {
                        let name_bytes = local_name.as_ref();
                        if name_bytes.windows(10).any(|w| w == b"GainMapMax") {
                            params.gain_map_max = f;
                            found_any = true;
                        } else if name_bytes.windows(10).any(|w| w == b"GainMapMin") {
                            params.gain_map_min = f;
                            found_any = true;
                        } else if name_bytes.windows(5).any(|w| w == b"Gamma") {
                            params.gamma = f;
                            found_any = true;
                        } else if name_bytes.windows(9).any(|w| w == b"OffsetSDR")
                            || name_bytes.windows(9).any(|w| w == b"OffsetSdr")
                        {
                            params.offset_sdr = f;
                            found_any = true;
                        } else if name_bytes.windows(9).any(|w| w == b"OffsetHDR")
                            || name_bytes.windows(9).any(|w| w == b"OffsetHdr")
                        {
                            params.offset_hdr = f;
                            found_any = true;
                        }
                    }
                }
                let name_bytes = e.name();
                let name_ref = name_bytes.as_ref();
                if name_ref.windows(10).any(|w| w == b"GainMapMax") {
                    if let Ok(val) = reader.read_text(name_bytes)
                        && let Ok(text_cow) = reader.decoder().decode(val.as_ref())
                        && let Ok(f) = text_cow.parse::<f32>()
                    {
                        params.gain_map_max = f;
                        found_any = true;
                    }
                } else if name_ref.windows(10).any(|w| w == b"GainMapMin") {
                    if let Ok(val) = reader.read_text(name_bytes)
                        && let Ok(text_cow) = reader.decoder().decode(val.as_ref())
                        && let Ok(f) = text_cow.parse::<f32>()
                    {
                        params.gain_map_min = f;
                        found_any = true;
                    }
                } else if name_ref.windows(5).any(|w| w == b"Gamma")
                    && let Ok(val) = reader.read_text(name_bytes)
                    && let Ok(text_cow) = reader.decoder().decode(val.as_ref())
                    && let Ok(f) = text_cow.parse::<f32>()
                {
                    params.gamma = f;
                    found_any = true;
                }
            }
            Err(_) | Ok(Event::Eof) => break,
            _ => (),
        }
        buf.clear();
    }
    if found_any { Some(params) } else { None }
}

/// # Errors
/// Returns an error if the HDR synthesis fails due to invalid parameters or processing errors.
pub fn synthesize_hdr(
    sdr: &DynamicImage,
    gain: &DynamicImage,
    params: &GainMapParams,
    needs_p3_conversion: bool,
) -> Result<Vec<f32>> {
    use image::GenericImageView;
    let (width, height) = sdr.dimensions();
    let gain_resized_storage: DynamicImage;
    let gain_resized = if gain.dimensions() == (width, height) {
        gain
    } else {
        gain_resized_storage =
            gain.resize_exact(width, height, image::imageops::FilterType::Triangle);
        &gain_resized_storage
    };

    let total_pixels = usize::try_from(
        u64::from(width)
            .checked_mul(u64::from(height))
            .and_then(|px| px.checked_mul(3))
            .ok_or_else(|| anyhow!("Buffer overflow"))?,
    )?;
    let mut hdr_pixels = vec![0.0f32; total_pixels];

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

    for y in 0..height {
        for x in 0..width {
            let idx = (usize::try_from(y)? * usize::try_from(width)? + usize::try_from(x)?) * 3;
            let (r_norm, g_norm, b_norm) = match (&sdr_16, &sdr_8) {
                (Some(buf), _) => {
                    let p = buf.get_pixel(x, y);
                    (
                        f32::from(p.0[0]) / 65535.0,
                        f32::from(p.0[1]) / 65535.0,
                        f32::from(p.0[2]) / 65535.0,
                    )
                }
                (None, Some(buf)) => {
                    let p = buf.get_pixel(x, y);
                    (
                        f32::from(p.0[0]) / 255.0,
                        f32::from(p.0[1]) / 255.0,
                        f32::from(p.0[2]) / 255.0,
                    )
                }
                (None, None) => unreachable!(),
            };

            let r_lin = srgb_to_linear(r_norm);
            let g_lin = srgb_to_linear(g_norm);
            let b_lin = srgb_to_linear(b_norm);

            let apply_gain = |val_raw: f32, max_val: f32| -> f32 {
                let val_norm = val_raw / max_val;
                let gain_px_corrected = val_norm.powf(1.0 / params.gamma.max(0.1));
                let log2_gain = gain_px_corrected * (params.gain_map_max - params.gain_map_min)
                    + params.gain_map_min;
                log2_gain.exp2()
            };

            let g_val = match (&gain_16, &gain_8) {
                (Some(buf), _) => apply_gain(f32::from(buf.get_pixel(x, y).0[0]), 65535.0),
                (None, Some(buf)) => apply_gain(f32::from(buf.get_pixel(x, y).0[0]), 255.0),
                (None, None) => apply_gain(128.0, 255.0),
            };

            let r_hdr = (r_lin + params.offset_sdr).mul_add(g_val, -params.offset_hdr);
            let g_hdr = (g_lin + params.offset_sdr).mul_add(g_val, -params.offset_hdr);
            let b_hdr = (b_lin + params.offset_sdr).mul_add(g_val, -params.offset_hdr);

            if needs_p3_conversion {
                let r_srgb = 0.0001f32.mul_add(-b_hdr, 1.2249f32.mul_add(r_hdr, -(0.2247 * g_hdr)));
                let g_srgb = 0.0001f32.mul_add(b_hdr, (-0.0420f32).mul_add(r_hdr, 1.0419 * g_hdr));
                let b_srgb =
                    1.0983f32.mul_add(b_hdr, (-0.0197f32).mul_add(r_hdr, -(0.0786 * g_hdr)));
                hdr_pixels[idx] = r_srgb.max(0.0);
                hdr_pixels[idx + 1] = g_srgb.max(0.0);
                hdr_pixels[idx + 2] = b_srgb.max(0.0);
            } else {
                hdr_pixels[idx] = r_hdr.max(0.0);
                hdr_pixels[idx + 1] = g_hdr.max(0.0);
                hdr_pixels[idx + 2] = b_hdr.max(0.0);
            }
        }
    }
    Ok(hdr_pixels)
}

fn resolve_intensity_target(derived: f32) -> Option<u32> {
    if let Ok(ov) = env::var("MFB_JXL_INTENSITY_TARGET")
        && let Ok(v) = ov.parse::<f32>()
        && v.is_finite()
        && v > 0.0
    {
        return crate::numeric_cast::f32_to_u32_strict(v.round(), "intensity_target");
    }
    if !derived.is_finite() || derived <= 0.0 {
        return None;
    }
    let clamped = derived.clamp(
        crate::constants::HDR_INTENSITY_TARGET_MIN,
        crate::constants::HDR_INTENSITY_TARGET_MAX,
    );
    crate::numeric_cast::f32_to_u32_strict(clamped.round(), "intensity_target")
}

fn srgb_to_linear(v: f32) -> f32 {
    if v <= crate::constants::SRGB_LINEAR_THRESHOLD {
        v / crate::constants::SRGB_LINEAR_SLOPE
    } else {
        ((v + crate::constants::SRGB_GAMMA_OFFSET) / crate::constants::SRGB_GAMMA_SCALE)
            .powf(crate::constants::SRGB_GAMMA_EXP)
    }
}

fn linear_to_pq(linear: f32) -> f32 {
    let l = (linear * crate::constants::HDR_DIFFUSE_WHITE_NITS) / crate::constants::HDR_MAX_NITS;
    let l = l.clamp(0.0, 1.0);
    let lm = l.powf(crate::constants::PQ_M1);
    let num = crate::constants::PQ_C2.mul_add(lm, crate::constants::PQ_C1);
    let den = crate::constants::PQ_C3.mul_add(lm, 1.0);
    (num / den).powf(crate::constants::PQ_M2)
}

fn write_png16(pixels: &[f32], width: u32, height: u32, path: &Path) -> Result<()> {
    let mut buffer = ImageBuffer::new(width, height);
    for (x, y, pixel) in buffer.enumerate_pixels_mut() {
        let idx = (usize::try_from(y)? * usize::try_from(width)? + usize::try_from(x)?) * 3;
        let r =
            crate::numeric_cast::f32_to_u16_strict(linear_to_pq(pixels[idx]) * 65535.0, "hdr_r_pq")
                .ok_or_else(|| ImgQualityError::NumericError("HDR R-channel cast failed".into()))?;
        let g = crate::numeric_cast::f32_to_u16_strict(
            linear_to_pq(pixels[idx + 1]) * 65535.0,
            "hdr_g_pq",
        )
        .ok_or_else(|| ImgQualityError::NumericError("HDR G-channel cast failed".into()))?;
        let b = crate::numeric_cast::f32_to_u16_strict(
            linear_to_pq(pixels[idx + 2]) * 65535.0,
            "hdr_b_pq",
        )
        .ok_or_else(|| ImgQualityError::NumericError("HDR B-channel cast failed".into()))?;
        *pixel = image::Rgb([r, g, b]);
    }
    buffer.save(path).context("Failed to save 16-bit PNG")?;
    Ok(())
}

fn write_exr(pixels: &[f32], width: u32, height: u32, path: &Path) -> Result<()> {
    use exr::prelude::*;
    let w = usize::try_from(width)?;
    let h = usize::try_from(height)?;
    if pixels.len() < w * h * 3 {
        return Err(anyhow::anyhow!(ImgQualityError::NumericError(format!(
            "EXR export: pixels buffer too small (expected {}, got {})",
            w * h * 3,
            pixels.len()
        ))));
    }

    write_rgb_file(path, w, h, |x, y| {
        let idx = (y * w + x) * 3;
        (pixels[idx], pixels[idx + 1], pixels[idx + 2])
    })
    .context("Failed to write EXR")?;
    Ok(())
}

fn parse_gainmap_params_from_jpeg_xmp(data: &[u8]) -> Option<GainMapParams> {
    let xmp_blocks = crate::image_jpeg_analysis::extract_xmp_from_jpeg_data(data)?;
    for xmp_bytes in xmp_blocks {
        if let Some(params) = parse_gainmap_from_xmp(xmp_bytes.as_bytes()) {
            return Some(params);
        }
    }
    None
}

/// # Errors
/// Returns an error if the HDR decoding fails due to invalid input or processing errors.
pub fn decode_hdr_image_to_png16(
    input: &Path,
    hdr_info: &ColorInfo,
) -> Result<(PathBuf, tempfile::NamedTempFile)> {
    if !should_use_hdr_decode(hdr_info) {
        anyhow::bail!("Not HDR");
    }
    let temp_png = tempfile::Builder::new().suffix(".png").tempfile()?;
    let temp_path = temp_png.path().to_path_buf();
    let pix_fmt = get_hdr_pix_fmt(hdr_info);
    let output = crate::FfmpegBuilder::new()
        .overwrite()
        .input(input)
        .pix_fmt_str(pix_fmt)
        .frames_v(1)
        .output(&temp_path)
        .build()
        .output()?;
    if !output.status.success() {
        anyhow::bail!("FFmpeg failed");
    }
    Ok((temp_path, temp_png))
}

#[must_use]
pub fn color_info_to_cicp(info: &ColorInfo) -> Option<String> {
    let primaries = match info.color_primaries.as_deref() {
        Some("bt709") => 1,
        Some("bt2020") => 9,
        Some("display-p3") => 12,
        _ if info.color_transfer.as_deref() == Some("smpte2084") => 9,
        _ => return None,
    };
    let transfer = match info.color_transfer.as_deref() {
        Some("smpte2084") => 16,
        Some("arib-std-b67") => 18,
        Some("bt709") => 1,
        Some("srgb") => 13,
        _ if primaries == 9 => 16,
        _ => return None,
    };
    let matrix = match info.color_space.as_deref() {
        Some("bt2020nc") => 9,
        Some("bt709") => 1,
        Some("rgb") => 0,
        _ if primaries == 9 => 9,
        _ => i32::from(primaries == 1),
    };
    Some(format!("{primaries}-{transfer}-{matrix}"))
}

#[must_use]
pub fn should_use_hdr_decode(info: &ColorInfo) -> bool {
    info.is_hdr() || info.bit_depth.is_some_and(|d| d > 8)
}

#[must_use]
pub fn get_hdr_pix_fmt(info: &ColorInfo) -> &'static str {
    if should_use_hdr_decode(info) {
        "rgb48le"
    } else {
        "rgb24"
    }
}

/// # Errors
/// Returns an error if the HEVC bitstream extraction fails due to invalid input or processing errors.
pub fn extract_hevc_bitstream(input: &Path, temp_dir: &Path) -> Result<PathBuf> {
    let raw_hevc = temp_dir.join("raw.hevc");
    let status = crate::FfmpegBuilder::new()
        .overwrite()
        .input(input)
        .arg("-c:v")
        .arg("copy")
        .arg("-bsf:v")
        .arg("hevc_mp4toannexb")
        .arg("-an")
        .arg("-sn")
        .output(&raw_hevc)
        .build()
        .output()?;
    if !status.status.success() {
        anyhow::bail!("bitstream extraction failed");
    }
    Ok(raw_hevc)
}

/// # Errors
/// Returns an error if the DV RPU extraction fails due to invalid input or processing errors.
pub fn extract_dv_rpu(raw_hevc: &Path, temp_dir: &Path, dv_profile: Option<u8>) -> Result<PathBuf> {
    let rpu_path = temp_dir.join("rpu.bin");
    let output = crate::tool_builders::DoviBuilder::new()
        .mode("extract-rpu")
        .input(raw_hevc)
        .output(&rpu_path)
        .build()
        .output()?;
    if !output.status.success() {
        anyhow::bail!("dovi_tool failed");
    }
    if dv_profile == Some(7) {
        let converted_rpu = temp_dir.join("rpu_p81.bin");
        let conv_output = crate::tool_builders::DoviBuilder::new()
            .mode("convert")
            .arg("--discard")
            .input(&rpu_path)
            .output(&converted_rpu)
            .build()
            .output()?;
        if !conv_output.status.success() {
            anyhow::bail!("dovi_tool convert failed");
        }
        return Ok(converted_rpu);
    }
    Ok(rpu_path)
}

/// # Errors
/// Returns an error if the HDR10+ metadata extraction fails due to invalid input or processing errors.
pub fn extract_hdr10plus_metadata(raw_hevc: &Path, temp_dir: &Path) -> Result<PathBuf> {
    let json_path = temp_dir.join("hdr10plus.json");
    let output = crate::tool_builders::Hdr10PlusBuilder::new()
        .mode("extract")
        .input(raw_hevc)
        .output(&json_path)
        .build()
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr_lower = stderr.to_lowercase();
        if stderr_lower.contains("error:") && stderr_lower.contains("invalid") {
            crate::log_anomaly!(
                crate::static_logs::messages::LABEL_VIDEO,
                "hdr10plus_tool exact extract validation failed, trying fallback with --skip-validation"
            );
            let fb_output = crate::tool_builders::Hdr10PlusBuilder::new()
                .mode("extract")
                .skip_validation(true)
                .input(raw_hevc)
                .output(&json_path)
                .build()
                .output()?;
            if !fb_output.status.success() {
                anyhow::bail!("hdr10plus_tool extract fallback failed");
            }
        } else {
            anyhow::bail!("hdr10plus_tool extract failed: {stderr}");
        }
    }
    Ok(json_path)
}

#[must_use]
pub fn dv_x265_profile_string(dv_profile: Option<u8>, compat_id: Option<u8>) -> Option<String> {
    match dv_profile {
        Some(5) => Some("5.0".to_string()),
        Some(7) => Some("8.1".to_string()),
        Some(8) => Some(format!(
            "8.{}",
            compat_id.unwrap_or(crate::constants::DV_PROFILE8_DEFAULT_COMPAT_ID)
        )),
        _ => None,
    }
}

#[must_use]
pub fn color_info_to_ffmpeg_args(info: &ColorInfo) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(ref colorspace) = info.color_space {
        args.push("-colorspace".to_string());
        args.push(colorspace.clone());
    }
    if let Some(ref trc) = info.color_transfer {
        args.push("-color_trc".to_string());
        args.push(trc.clone());
    }
    if let Some(ref primaries) = info.color_primaries {
        args.push("-color_primaries".to_string());
        args.push(primaries.clone());
    }
    if let Some(ref range) = info.color_range {
        args.push("-color_range".to_string());
        args.push(range.clone());
    }
    args
}

#[must_use]
pub fn color_info_to_x265_hdr_params(info: &ColorInfo) -> Option<String> {
    if !info.is_hdr() {
        return None;
    }
    let mut params = Vec::new();
    if let Some(ref primaries) = info.color_primaries {
        let code = match primaries.as_str() {
            "bt709" => "1",
            "smpte432" | "display-p3" => "12",
            _ => "9",
        };
        params.push(format!("colorprim={code}"));
    }
    if let Some(ref trc) = info.color_transfer {
        let code = match trc.as_str() {
            "arib-std-b67" => "18",
            "bt709" => "1",
            _ => "16",
        };
        params.push(format!("transfer={code}"));
    }
    if let Some(ref colorspace) = info.color_space {
        let code = match colorspace.as_str() {
            "bt709" => "1",
            _ => "9",
        };
        params.push(format!("colormatrix={code}"));
    }
    if let Some(ref master) = info.mastering_display {
        params.push(format!("master-display={master}"));
    }
    if let Some(ref cll) = info.max_cll {
        params.push(format!("max-cll={cll}"));
    }
    if params.is_empty() {
        None
    } else {
        Some(params.join(":"))
    }
}

#[must_use]
pub fn infer_bt709_if_modern(mut info: ColorInfo, width: u32, height: u32, ext: &str) -> ColorInfo {
    let is_hd = width >= 1280 || height >= 720;
    let is_modern_format = matches!(
        ext.to_lowercase().as_str(),
        "avif" | "webp" | "jxl" | "heic" | "heif" | "apng"
    );
    if info.is_hdr() || info.bit_depth.is_some_and(|d| d > 8) {
        return info;
    }
    if is_hd || is_modern_format {
        if info.color_space.is_none() {
            info.color_space = Some("bt709".to_string());
        }
        if info.color_transfer.is_none() {
            info.color_transfer = Some("iec61966-2-1".to_string());
        }
        if info.color_primaries.is_none() {
            info.color_primaries = Some("bt709".to_string());
        }
    }
    info
}

#[must_use]
pub fn is_dovi_tool_available() -> bool {
    crate::tool_builders::DoviBuilder::check_available()
}

#[must_use]
pub fn is_hdr10plus_tool_available() -> bool {
    crate::tool_builders::Hdr10PlusBuilder::check_available()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cicp_hdr10() {
        let info = ColorInfo {
            color_primaries: Some("bt2020".to_string()),
            color_transfer: Some("smpte2084".to_string()),
            color_space: Some("bt2020nc".to_string()),
            ..Default::default()
        };
        assert_eq!(color_info_to_cicp(&info), Some("9-16-9".to_string()));
    }

    #[test]
    fn test_cicp_hlg() {
        let info = ColorInfo {
            color_primaries: Some("bt2020".to_string()),
            color_transfer: Some("arib-std-b67".to_string()),
            color_space: Some("bt2020nc".to_string()),
            ..Default::default()
        };
        assert_eq!(color_info_to_cicp(&info), Some("9-18-9".to_string()));
    }

    #[test]
    fn test_cicp_sdr() {
        let info = ColorInfo {
            color_primaries: Some("bt709".to_string()),
            color_transfer: Some("bt709".to_string()),
            color_space: Some("bt709".to_string()),
            ..Default::default()
        };
        assert_eq!(color_info_to_cicp(&info), Some("1-1-1".to_string()));
    }

    #[test]
    fn test_cicp_no_metadata() {
        let info = ColorInfo::default();
        assert_eq!(color_info_to_cicp(&info), None);
    }

    #[test]
    fn test_ffmpeg_args() {
        let info = ColorInfo {
            color_primaries: Some("bt2020".to_string()),
            color_transfer: Some("smpte2084".to_string()),
            color_space: Some("bt2020nc".to_string()),
            ..Default::default()
        };
        let args = color_info_to_ffmpeg_args(&info);
        assert_eq!(
            args,
            vec![
                "-colorspace",
                "bt2020nc",
                "-color_trc",
                "smpte2084",
                "-color_primaries",
                "bt2020"
            ]
        );
    }

    #[test]
    fn test_should_use_hdr_decode() {
        let hdr_info = ColorInfo {
            color_transfer: Some("smpte2084".to_string()),
            bit_depth: Some(10),
            ..Default::default()
        };
        assert!(should_use_hdr_decode(&hdr_info));

        let sdr_info = ColorInfo {
            bit_depth: Some(8),
            ..Default::default()
        };
        assert!(!should_use_hdr_decode(&sdr_info));
    }

    #[test]
    fn test_get_hdr_pix_fmt() {
        let hdr_info = ColorInfo {
            bit_depth: Some(10),
            ..Default::default()
        };
        assert_eq!(get_hdr_pix_fmt(&hdr_info), "rgb48le");

        let sdr_info = ColorInfo {
            bit_depth: Some(8),
            ..Default::default()
        };
        assert_eq!(get_hdr_pix_fmt(&sdr_info), "rgb24");
    }
}
