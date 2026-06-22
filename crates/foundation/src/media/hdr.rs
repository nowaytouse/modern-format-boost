//! # HDR and Color Space unification
//!
//! Consolidated module for HDR decoding, synthesis, and color space utilities.

use anyhow::{Context, Result, anyhow};
use image::{DynamicImage, ImageBuffer, ImageFormat};
use libheif_rs::{ColorSpace, HeifContext, ImageHandle, ItemId, RgbChroma};
use quick_xml::events::Event;
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};

use crate::builder_base::ToolBuilder;
use crate::ffprobe_json::ColorInfo;
use crate::image_jpeg_analysis::extract_ultrahdr_jpeg_payload;
use crate::jxl_builder::CjxlBuilder;
use crate::unified_error::ImgQualityError;

fn read_native_u16_word(data: &[u8], word_index: usize) -> Option<u16> {
    let byte_index = word_index.checked_mul(2)?;
    let bytes = data.get(byte_index..byte_index + 2)?;
    Some(u16::from_ne_bytes([bytes[0], bytes[1]]))
}

/// HDR intermediate format selection
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IntermediateFormat {
    /// 32-bit float `OpenEXR` - maximum precision
    #[default]
    OpenExr32,
    /// 16-bit integer PNG - high precision with better compatibility
    Png16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HdrSidecarArtifact {
    suffix: &'static str,
    extension: &'static str,
    data: Vec<u8>,
    description: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HdrAuxiliaryRole {
    GainMap,
    Depth,
    Other,
}

fn classify_hdr_auxiliary_type(aux_type: &str) -> HdrAuxiliaryRole {
    let aux_type = aux_type.to_ascii_lowercase();
    if aux_type.contains("hdrgainmap") || aux_type.contains("gainmap") {
        HdrAuxiliaryRole::GainMap
    } else if aux_type.contains("depth") || aux_type.contains("auxiliarydepth") {
        HdrAuxiliaryRole::Depth
    } else {
        HdrAuxiliaryRole::Other
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HdrArtifacts {
    sidecars: Vec<HdrSidecarArtifact>,
}

impl HdrArtifacts {
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.sidecars.is_empty()
    }

    #[must_use]
    pub const fn sidecar_count(&self) -> usize {
        self.sidecars.len()
    }

    fn push_raw_sidecar(
        &mut self,
        suffix: &'static str,
        extension: &'static str,
        data: Vec<u8>,
        description: &'static str,
    ) {
        self.sidecars.push(HdrSidecarArtifact {
            suffix,
            extension,
            data,
            description,
        });
    }
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
///
/// Returns any HDR sidecar artifacts that must be committed next to the final
/// output after the synthesized JXL itself has been finalized.
///
/// # Errors
/// Returns an error if the conversion fails due to invalid input or processing errors.
pub fn convert_heic_with_gainmap_to_jxl(
    input: &Path,
    output: &Path,
    apple_compat: bool,
    intermediate_format: IntermediateFormat,
    ultimate: bool,
    archive: bool,
) -> Result<HdrArtifacts> {
    let actual_distance = crate::constants::jxl_distance_for_mode(1.0, ultimate);
    let actual_effort =
        crate::jxl_effort_policy::direct_encode_effort_for_archive(archive, ultimate);

    let file_label = crate::media_conversion_gate::probe_hdr_heic_input_label(input);

    crate::log_info!(
        crate::infra::static_logs::messages::LABEL_HDR_SYNTHESIS,
        crate::infra::static_logs::messages::MSG_HDR_INIT.replace("{}", &file_label)
    );

    let data = std::fs::read(input).context("Failed to read HEIC file")?;
    let ctx = HeifContext::read_from_bytes(&data).context("Failed to parse HEIC context")?;
    let handle = ctx
        .primary_image_handle()
        .map_err(|e| anyhow!("Failed to get primary image handle: {e}"))?;

    let needs_p3_conversion = is_display_p3(&data);

    let aux_images = handle.auxiliary_images(None);
    let mut gainmap_item_id: Option<libheif_rs::ItemId> = None;

    for aux in &aux_images {
        let aux_type = aux.auxiliary_type().map_err(|e| {
            anyhow!("Failed to read HEIC auxiliary image type for gainmap scan: {e}")
        })?;
        if classify_hdr_auxiliary_type(&aux_type) == HdrAuxiliaryRole::GainMap {
            gainmap_item_id = Some(aux.item_id());
            break;
        }
    }

    let gainmap_item =
        gainmap_item_id.ok_or_else(|| anyhow!("No gainmap found in auxiliary images"))?;

    let gain_handle = ctx
        .image_handle(gainmap_item)
        .map_err(|e| anyhow!("Failed to get gainmap handle: {e}"))?;

    let mut depth_item_id: Option<libheif_rs::ItemId> = None;
    for aux in &aux_images {
        let aux_type = aux
            .auxiliary_type()
            .map_err(|e| anyhow!("Failed to read HEIC auxiliary image type for depth scan: {e}"))?;
        if classify_hdr_auxiliary_type(&aux_type) == HdrAuxiliaryRole::Depth {
            depth_item_id = Some(aux.item_id());
            break;
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

    let params = parse_gainmap_params(&handle)?
        .ok_or_else(|| anyhow!("Failed to parse gainmap parameters from HEIC XMP metadata"))?;

    crate::log_detail!(
        &crate::infra::static_logs::messages::MSG_HDR_SYNTH_ACTIVE
            .replacen("{}", &needs_p3_conversion.to_string(), 1)
            .replacen("{}", &file_label, 1)
    );

    let hdr_pixels = synthesize(&sdr, &gain, &params, needs_p3_conversion)
        .context("☢️ HDR synthesis math failure")?;

    let (tmp_file, intensity_target) = match intermediate_format {
        IntermediateFormat::OpenExr32 => {
            let tmp_exr = output.with_extension("tmp_hdr.exr");
            write_exr(&hdr_pixels, sdr.width(), sdr.height(), &tmp_exr)
                .context("Failed to write intermediate 32-bit OpenEXR buffer")?;
            (
                tmp_exr,
                f64::from(crate::constants::HDR_REFERENCE_WHITE_NITS)
                    * f64::from(params.gain_map_max.exp2()),
            )
        }
        IntermediateFormat::Png16 => {
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
                crate::infra::static_logs::messages::LABEL_CALIBRATION,
                format!("Applying intensity_target {it} for HDR synthesis")
            );
        } else {
            crate::media_conversion_gate::hdr_intensity_target_audit(
                "hdr_intensity_target_missing",
                "No valid intensity_target — proceeding without --intensity_target",
            );
        }
    } else {
        crate::media_conversion_gate::hdr_intensity_target_audit(
            "hdr_intensity_target_invalid_float",
            "Invalid intensity_target float conversion — proceeding without --intensity_target",
        );
    }

    let status = builder
        .build()
        .status()
        .context("Failed to execute cjxl for HDR synthesis")?;

    if !status.success() {
        if tmp_file.exists() {
            crate::media_conversion_gate::delivery_remove_file_or_audit(
                "hdr_synthesis_tmp_cleanup",
                &tmp_file,
            );
        }
        return Err(anyhow!(
            "cjxl encoding failed with status {status} during HDR synthesis"
        ));
    }

    let mut artifacts = HdrArtifacts::default();
    if let Some(depth) = depth_image {
        let depth_png = encode_png_sidecar_bytes(&DynamicImage::ImageLuma16(depth.to_luma16()))
            .context("Failed to encode depth sidecar PNG")?;
        artifacts.push_raw_sidecar("depth", "png", depth_png, "HDR depth map PNG sidecar");
    }

    if tmp_file.exists() {
        crate::media_conversion_gate::delivery_remove_file_or_audit(
            "hdr_synthesis_tmp_cleanup",
            &tmp_file,
        );
    }

    Ok(artifacts)
}

/// Returns any HDR sidecar artifacts that must be committed next to the final
/// output after the synthesized JXL itself has been finalized.
///
/// # Errors
/// Returns an error if the conversion fails due to invalid input or processing errors.
pub fn convert_ultrahdr_jpeg_to_jxl(
    input: &Path,
    output: &Path,
    apple_compat: bool,
    intermediate_format: IntermediateFormat,
    ultimate: bool,
    archive: bool,
) -> Result<HdrArtifacts> {
    let actual_distance = crate::constants::jxl_distance_for_mode(1.0, ultimate);
    let actual_effort =
        crate::jxl_effort_policy::direct_encode_effort_for_archive(archive, ultimate);

    log_info!(
        crate::infra::static_logs::messages::LABEL_CONVERSION,
        &format!(
            "UltraHDR JPEG HDR synthesis started for: {}",
            input.display()
        )
    );

    let data = std::fs::read(input).context("Failed to read UltraHDR JPEG file")?;

    let extracted = extract_ultrahdr_jpeg_payload(&data)
        .map_err(|e| anyhow!("☢️ Failed to extract gainmap from UltraHDR JPEG: {e}"))?;
    let crate::image_jpeg_analysis::UltraHdrJpegPayload {
        base_image,
        gainmap_image,
        gainmap_jpeg,
    } = extracted;

    crate::log_detail!(
        &crate::infra::static_logs::messages::MSG_GAINMAP_EXTRACTED
            .replacen(
                "{}",
                &format!("{}x{}", gainmap_image.width(), gainmap_image.height()),
                1
            )
            .replacen(
                "{}",
                &format!("{}x{}", base_image.width(), base_image.height()),
                1
            )
    );

    let needs_p3_conversion = false;

    let params = parse_gainmap_params_from_jpeg_xmp(&data)?
        .ok_or_else(|| anyhow::anyhow!("No valid XMP gainmap parameters found in the image"))?;

    crate::log_info!(
        crate::infra::static_logs::messages::LABEL_GAINMAP_AUDIT,
        crate::infra::static_logs::messages::MSG_GAINMAP_PARAMS
            .replacen("{}", &format!("{:.2}", params.gain_map_max), 1)
            .replacen("{}", &format!("{:.2}", params.gain_map_min), 1)
            .replacen("{}", &format!("{:.2}", params.gamma), 1)
    );

    let hdr_pixels = synthesize(&base_image, &gainmap_image, &params, needs_p3_conversion)
        .context("☢️ HDR synthesis math failure")?;

    let (tmp_file, intensity_target) = match intermediate_format {
        IntermediateFormat::OpenExr32 => {
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
        IntermediateFormat::Png16 => {
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
                crate::infra::static_logs::messages::LABEL_CALIBRATION,
                &format!("Applying intensity_target {it} for UltraHDR synthesis")
            );
        } else {
            crate::media_conversion_gate::hdr_intensity_target_audit(
                "hdr_intensity_target_missing",
                "No valid intensity_target — proceeding without --intensity_target",
            );
        }
    } else {
        crate::media_conversion_gate::hdr_intensity_target_audit(
            "hdr_intensity_target_invalid_float",
            "Invalid intensity_target float conversion — proceeding without --intensity_target",
        );
    }

    let status = builder
        .build()
        .status()
        .context("Failed to execute cjxl for HDR synthesis")?;

    if !status.success() {
        if tmp_file.exists() {
            crate::media_conversion_gate::delivery_remove_file_or_audit(
                "hdr_ultrahdr_tmp_cleanup",
                &tmp_file,
            );
        }
        return Err(anyhow!(
            "cjxl encoding failed during UltraHDR JPEG HDR synthesis"
        ));
    }

    if tmp_file.exists() {
        crate::media_conversion_gate::delivery_remove_file_or_audit(
            "hdr_ultrahdr_tmp_cleanup",
            &tmp_file,
        );
    }

    let mut artifacts = HdrArtifacts::default();
    artifacts.push_raw_sidecar(
        "gainmap",
        "jpg",
        gainmap_jpeg,
        "Raw UltraHDR gainmap JPEG sidecar",
    );

    Ok(artifacts)
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
    archive: bool,
) -> Result<()> {
    let actual_distance = crate::constants::jxl_distance_for_mode(distance, ultimate);
    let actual_effort =
        crate::jxl_effort_policy::direct_encode_effort_for_archive(archive, ultimate);

    log_info!(
        crate::infra::static_logs::messages::LABEL_CONVERSION,
        &format!(
            "📤 UltraHDR JPEG Gainmap migration started: {}",
            input.display()
        )
    );

    let data = std::fs::read(input).context("Failed to read UltraHDR JPEG file")?;
    let extracted = extract_ultrahdr_jpeg_payload(&data)
        .map_err(|e| anyhow!("Failed to extract gainmap for migration: {e}"))?;

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

    crate::metadata::preserve_for_delivery(input, output)
        .context("Failed to preserve metadata for UltraHDR migration output")?;

    let mut artifacts = HdrArtifacts::default();
    artifacts.push_raw_sidecar(
        "gainmap",
        "jpg",
        extracted.gainmap_jpeg,
        "Raw UltraHDR gainmap JPEG sidecar",
    );
    persist_hdr_artifacts(output, &artifacts)
        .context("Failed to preserve raw UltraHDR gainmap sidecar")?;

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
    let slice =
        crate::media_conversion_gate::probe_buffer_prefix_or_empty(data, end, "hdr icc scan");
    slice.windows(10).any(|w| w == b"Display P3")
        || slice.windows(2).any(|w| w == b"P3") && slice.windows(4).any(|w| w == b"colr")
}

fn parse_gainmap_params(handle: &ImageHandle) -> Result<Option<GainMapParams>> {
    let mut ids = [ItemId::default(); 1];
    let count = handle.metadata_block_ids(&mut ids, b"xmp ");
    if count == 0 {
        return Ok(None);
    }
    let xmp_data = handle
        .metadata(ids[0])
        .map_err(|e| anyhow!("Failed to read gainmap XMP metadata: {e}"))?;
    parse_gainmap_from_xmp(&xmp_data)
}

fn parse_gainmap_from_xmp(xmp_data: &[u8]) -> Result<Option<GainMapParams>> {
    let mut params = GainMapParams::default();
    let mut reader = quick_xml::reader::Reader::from_reader(xmp_data);
    reader.config_mut().trim_text(true);
    let mut found_any = false;

    loop {
        match reader.read_event() {
            Ok(Event::Start(e) | Event::Empty(e)) => {
                for attr in e.attributes().flatten() {
                    let local_name = attr.key.local_name();
                    let name_bytes = local_name.as_ref();
                    let target = if name_bytes.windows(10).any(|w| w == b"GainMapMax") {
                        Some("GainMapMax")
                    } else if name_bytes.windows(10).any(|w| w == b"GainMapMin") {
                        Some("GainMapMin")
                    } else if name_bytes.windows(5).any(|w| w == b"Gamma") {
                        Some("Gamma")
                    } else if name_bytes.windows(9).any(|w| w == b"OffsetSDR")
                        || name_bytes.windows(9).any(|w| w == b"OffsetSdr")
                    {
                        Some("OffsetSDR")
                    } else if name_bytes.windows(9).any(|w| w == b"OffsetHDR")
                        || name_bytes.windows(9).any(|w| w == b"OffsetHdr")
                    {
                        Some("OffsetHDR")
                    } else {
                        None
                    };
                    let Some(target) = target else {
                        continue;
                    };
                    let unescaped = attr
                        .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                        .map_err(|e| {
                            anyhow!("Failed to normalize gainmap XMP attr {target}: {e}")
                        })?;
                    let f = unescaped.as_ref().parse::<f32>().map_err(|e| {
                        anyhow!(
                            "Failed to parse gainmap XMP attr {target} value {:?}: {e}",
                            unescaped.as_ref()
                        )
                    })?;
                    match target {
                        "GainMapMax" => params.gain_map_max = f,
                        "GainMapMin" => params.gain_map_min = f,
                        "Gamma" => params.gamma = f,
                        "OffsetSDR" => params.offset_sdr = f,
                        "OffsetHDR" => params.offset_hdr = f,
                        _ => unreachable!("target enumerated above"),
                    }
                    found_any = true;
                }
                let name = e.name();
                let name_ref = name.as_ref();
                if name_ref.windows(10).any(|w| w == b"GainMapMax") {
                    let val = reader.read_text(name)?;
                    let raw = String::from_utf8_lossy(val.as_ref());
                    params.gain_map_max = raw.parse::<f32>().map_err(|e| {
                        anyhow!("Failed to parse GainMapMax text value {raw:?}: {e}")
                    })?;
                    found_any = true;
                } else if name_ref.windows(10).any(|w| w == b"GainMapMin") {
                    let val = reader.read_text(name)?;
                    let raw = String::from_utf8_lossy(val.as_ref());
                    params.gain_map_min = raw.parse::<f32>().map_err(|e| {
                        anyhow!("Failed to parse GainMapMin text value {raw:?}: {e}")
                    })?;
                    found_any = true;
                } else if name_ref.windows(5).any(|w| w == b"Gamma") {
                    let val = reader.read_text(name)?;
                    let raw = String::from_utf8_lossy(val.as_ref());
                    params.gamma = raw
                        .parse::<f32>()
                        .map_err(|e| anyhow!("Failed to parse Gamma text value {raw:?}: {e}"))?;
                    found_any = true;
                }
            }
            Err(err) => return Err(anyhow!("Failed to parse gainmap XMP: {err}")),
            Ok(Event::Eof) => break,
            _ => (),
        }
    }
    Ok(if found_any { Some(params) } else { None })
}

/// # Errors
/// Returns an error if the HDR synthesis fails due to invalid parameters or processing errors.
pub fn synthesize(
    sdr: &DynamicImage,
    gain: &DynamicImage,
    params: &GainMapParams,
    needs_p3_conversion: bool,
) -> Result<Vec<f32>> {
    use image::GenericImageView;
    let (width, height) = sdr.dimensions();
    let mut gain_resized_storage: DynamicImage;
    let gain_resized = if gain.dimensions() == (width, height) {
        gain
    } else {
        gain_resized_storage = gain.clone();
        gain_resized_storage.resize_exact(width, height, image::imageops::FilterType::Triangle);
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
    match env::var(crate::constants::ENV_JXL_INTENSITY_TARGET) {
        Ok(ov) => match ov.parse::<f32>() {
            Ok(v) if v.is_finite() && v > 0.0 => {
                return crate::numeric_cast::f32_to_u32_strict(v.round(), "intensity_target");
            }
            Ok(v) => {
                crate::media_conversion_gate::delivery_jxl_batch_audit(
                    "hdr_intensity_target_env",
                    format!("invalid non-positive/non-finite intensity target override: {v}"),
                );
            }
            Err(err) => {
                crate::media_conversion_gate::delivery_jxl_batch_audit(
                    "hdr_intensity_target_env",
                    format!("malformed intensity target override {ov:?}: {err}"),
                );
            }
        },
        Err(env::VarError::NotPresent) => {}
        Err(err) => {
            crate::media_conversion_gate::delivery_jxl_batch_audit(
                "hdr_intensity_target_env",
                format!(
                    "{} could not be read: {err}",
                    crate::constants::ENV_JXL_INTENSITY_TARGET
                ),
            );
        }
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

fn sidecar_output_path(output: &Path, suffix: &str, extension: &str) -> PathBuf {
    output.with_extension(format!("{suffix}.{extension}"))
}

fn encode_png_sidecar_bytes(image: &DynamicImage) -> Result<Vec<u8>> {
    let mut cursor = Cursor::new(Vec::new());
    image
        .write_to(&mut cursor, ImageFormat::Png)
        .context("Failed to encode PNG sidecar bytes")?;
    Ok(cursor.into_inner())
}

fn persist_sidecar(path: &Path, data: &[u8]) -> Result<()> {
    if data.is_empty() {
        return Err(anyhow!(
            "Refusing to write empty sidecar: {}",
            path.display()
        ));
    }

    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("Sidecar path has no parent: {}", path.display()))?;
    let suffix = format!(
        ".{}",
        crate::media_conversion_gate::probe_hdr_sidecar_extension_or_bin(path)
    );
    let mut temp_file = crate::media_conversion_gate::delivery_named_tempfile_in_parent_or_err(
        "hdr_sidecar_stage",
        parent,
        ".mfb-sidecar-",
        &suffix,
    )
    .with_context(|| format!("Failed to create temporary sidecar near {}", path.display()))?;
    temp_file
        .write_all(data)
        .with_context(|| format!("Failed to stage sidecar bytes for {}", path.display()))?;
    temp_file
        .flush()
        .with_context(|| format!("Failed to flush staged sidecar {}", path.display()))?;

    if path.exists() {
        fs::remove_file(path)
            .with_context(|| format!("Failed to replace existing sidecar {}", path.display()))?;
    }

    temp_file
        .persist(path)
        .map_err(|e| anyhow!("Failed to persist sidecar {}: {}", path.display(), e.error))?;
    Ok(())
}

/// Persist HDR sidecar artifacts next to the finalized output.
///
/// # Errors
///
/// Returns an error if any artifact cannot be written. Any partially written
/// sidecars from this call are cleaned up before returning.
pub fn persist_hdr_artifacts(output: &Path, artifacts: &HdrArtifacts) -> Result<Vec<PathBuf>> {
    let mut written_paths: Vec<PathBuf> = Vec::new();

    for artifact in &artifacts.sidecars {
        let sidecar_path = sidecar_output_path(output, artifact.suffix, artifact.extension);
        if let Err(err) = persist_sidecar(&sidecar_path, &artifact.data) {
            for written in &written_paths {
                if let Err(cleanup_err) = fs::remove_file(written)
                    && cleanup_err.kind() != std::io::ErrorKind::NotFound
                {
                    crate::media_conversion_gate::delivery_cleanup_audit(
                        written,
                        "hdr_sidecar_partial_cleanup",
                        cleanup_err,
                    );
                }
            }
            return Err(err);
        }

        crate::log_success!(
            crate::infra::static_logs::messages::LABEL_GAINMAP_AUDIT,
            &format!(
                "{} preserved: {}",
                artifact.description,
                sidecar_path.display()
            )
        );
        written_paths.push(sidecar_path);
    }

    Ok(written_paths)
}

fn parse_gainmap_params_from_jpeg_xmp(data: &[u8]) -> Result<Option<GainMapParams>> {
    let Some(xmp_blocks) = crate::image_jpeg_analysis::extract_xmp_from_jpeg_data(data) else {
        return Ok(None);
    };
    for xmp_bytes in xmp_blocks {
        if let Some(params) = parse_gainmap_from_xmp(xmp_bytes.as_bytes())? {
            return Ok(Some(params));
        }
    }
    Ok(None)
}

/// # Errors
/// Returns an error if the input does not require high-precision `PNG16`
/// `preservation` or if `FFmpeg` processing fails.
pub fn decode_image_to_png16_preserving_precision(
    input: &Path,
    color_info: &ColorInfo,
) -> Result<(PathBuf, tempfile::NamedTempFile)> {
    let ext_lower =
        crate::media_conversion_gate::path_extension_lowercase_or_empty_unchecked(input);
    let ext_lower = (!ext_lower.is_empty()).then_some(ext_lower.as_str());
    let precision = crate::media_precision::ImagePrecisionProfile::from_media_context(
        ext_lower, color_info, None,
    );
    if !precision.should_use_high_precision_png16_decode() {
        anyhow::bail!("Input does not require high-precision PNG16 decode");
    }
    let temp_png = crate::media_conversion_gate::delivery_named_tempfile_in_scratch_or_err(
        "hdr_png16_decode",
        None,
        Some(".png"),
    )?;
    let temp_path = temp_png.path().to_path_buf();
    let pix_fmt = crate::media_conversion_gate::precision_png16_decode_rgb_pix_fmt(&precision);
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
            crate::media_conversion_gate::hdr_intensity_target_audit(
                "hdr10plus_extract_validation_fallback",
                "hdr10plus_tool exact extract validation failed, trying fallback with --skip-validation",
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
            crate::media_conversion_gate::dv_profile8_compat_id_or_default(compat_id)
        )),
        _ => None,
    }
}

#[must_use]
fn sanitized_ffmpeg_color_value(value: Option<&str>) -> Option<&str> {
    value
        .map(str::trim)
        .filter(|candidate| !candidate.is_empty() && *candidate != crate::constants::STR_UNKNOWN)
}

#[must_use]
fn normalized_yuv_output_colorspace(value: Option<&str>) -> Option<String> {
    let colorspace = sanitized_ffmpeg_color_value(value)?;
    let normalized = match colorspace {
        "bt2020ncl" | "bt2020_ncl" => crate::constants::CS_BT2020,
        "bt2020cl" | "bt2020_cl" => "bt2020c",
        crate::constants::CS_GBR | crate::constants::CS_RGB | crate::constants::CS_GBRP => {
            return None;
        }
        other => other,
    };
    Some(normalized.to_string())
}

#[must_use]
fn build_ffmpeg_color_args_impl(
    colorspace: Option<String>,
    color_transfer: Option<&str>,
    color_primaries: Option<&str>,
    color_range: Option<&str>,
) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(colorspace) = colorspace {
        args.push("-colorspace".to_string());
        args.push(colorspace);
    }
    if let Some(trc) = sanitized_ffmpeg_color_value(color_transfer) {
        args.push("-color_trc".to_string());
        args.push(trc.to_string());
    }
    if let Some(primaries) = sanitized_ffmpeg_color_value(color_primaries) {
        args.push("-color_primaries".to_string());
        args.push(primaries.to_string());
    }
    if let Some(range) = sanitized_ffmpeg_color_value(color_range) {
        args.push("-color_range".to_string());
        args.push(range.to_string());
    }
    args
}

#[must_use]
pub fn build_yuv_output_ffmpeg_color_args(
    color_space: Option<&str>,
    color_transfer: Option<&str>,
    color_primaries: Option<&str>,
) -> Vec<String> {
    // Only emit the container-level CICP triple here. HDR10-family signaling
    // still belongs in encoder-specific parameter sets such as `-x265-params`.
    build_ffmpeg_color_args_impl(
        normalized_yuv_output_colorspace(color_space),
        color_transfer,
        color_primaries,
        None,
    )
}

#[must_use]
pub fn color_info_to_ffmpeg_args(info: &ColorInfo) -> Vec<String> {
    build_ffmpeg_color_args_impl(
        sanitized_ffmpeg_color_value(info.color_space.as_deref()).map(str::to_owned),
        info.color_transfer.as_deref(),
        info.color_primaries.as_deref(),
        info.color_range.as_deref(),
    )
}

#[must_use]
pub fn color_info_to_x265_hdr_params(info: &ColorInfo) -> Option<String> {
    if !should_emit_x265_hdr10_metadata(
        info.color_space.as_deref(),
        info.color_transfer.as_deref(),
        info.color_primaries.as_deref(),
        info.mastering_display.as_deref(),
        info.max_cll.as_deref(),
        info.is_hdr10_plus,
    ) {
        return None;
    }
    let mut params = String::new();
    if let Some(ref primaries) = info.color_primaries {
        let code = match primaries.as_str() {
            "bt709" => "1",
            "smpte432" | "display-p3" => "12",
            _ => "9",
        };
        crate::x265_params::push_param(&mut params, &format!("colorprim={code}"));
    }
    if let Some(ref trc) = info.color_transfer {
        let code = match trc.as_str() {
            "arib-std-b67" => "18",
            "bt709" => "1",
            _ => "16",
        };
        crate::x265_params::push_param(&mut params, &format!("transfer={code}"));
    }
    if let Some(ref colorspace) = info.color_space {
        let code = match colorspace.as_str() {
            "bt709" => "1",
            _ => "9",
        };
        crate::x265_params::push_param(&mut params, &format!("colormatrix={code}"));
    }

    append_x265_hdr10_params(
        &mut params,
        info.color_space.as_deref(),
        info.color_transfer.as_deref(),
        info.color_primaries.as_deref(),
        info.mastering_display.as_deref(),
        info.max_cll.as_deref(),
        info.is_hdr10_plus,
        crate::media_precision::hevc_yuv420_output_pix_fmt(info),
        None,
    );

    (!params.is_empty()).then_some(params)
}

fn has_nonempty_value(value: Option<&str>) -> bool {
    value.is_some_and(|candidate| !candidate.trim().is_empty())
}

fn has_x265_hdr10_triplet(
    color_space: Option<&str>,
    color_transfer: Option<&str>,
    color_primaries: Option<&str>,
) -> bool {
    let color_space_lower = color_space.map(str::to_ascii_lowercase);
    let color_transfer_lower = color_transfer.map(str::to_ascii_lowercase);
    let color_primaries_lower = color_primaries.map(str::to_ascii_lowercase);

    color_primaries_lower.as_deref() == Some("bt2020")
        && color_transfer_lower.as_deref() == Some(crate::constants::HDR_TRANSFER_PQ)
        && matches!(
            color_space_lower.as_deref(),
            Some("bt2020" | "bt2020nc" | "bt2020ncl")
        )
}

fn has_hdr10_like_signal(
    color_transfer: Option<&str>,
    mastering_display: Option<&str>,
    max_cll: Option<&str>,
    is_hdr10_plus: bool,
) -> bool {
    let color_transfer_lower = color_transfer.map(str::to_ascii_lowercase);

    is_hdr10_plus
        || has_nonempty_value(mastering_display)
        || has_nonempty_value(max_cll)
        || color_transfer_lower.as_deref() == Some(crate::constants::HDR_TRANSFER_PQ)
}

/// Returns true when HDR10-family x265 metadata may be safely emitted.
///
/// This is intentionally stricter than generic HDR detection. HLG and BT.2020 SDR are excluded
/// because x265's HDR10 signaling parameters are only valid for BT.2020 + PQ style output.
#[must_use]
pub fn should_emit_x265_hdr10_metadata(
    color_space: Option<&str>,
    color_transfer: Option<&str>,
    color_primaries: Option<&str>,
    mastering_display: Option<&str>,
    max_cll: Option<&str>,
    is_hdr10_plus: bool,
) -> bool {
    has_hdr10_like_signal(color_transfer, mastering_display, max_cll, is_hdr10_plus)
        && has_x265_hdr10_triplet(color_space, color_transfer, color_primaries)
}

/// Returns true only for HDR10-like output that can benefit from x265's HDR10 optimization.
///
/// This intentionally excludes HLG and BT.2020 SDR. The optimization is only appropriate when the
/// encoded output is 10-bit 4:2:0 with BT.2020 primaries, a BT.2020 matrix, and PQ transfer.
#[must_use]
pub fn should_enable_x265_hdr10_opt(
    color_space: Option<&str>,
    color_transfer: Option<&str>,
    color_primaries: Option<&str>,
    mastering_display: Option<&str>,
    max_cll: Option<&str>,
    is_hdr10_plus: bool,
    output_pix_fmt: &str,
) -> bool {
    let has_hdr10_sampling = crate::ffprobe::detect_bit_depth(output_pix_fmt) == Some(10)
        && output_pix_fmt.contains("420");

    should_emit_x265_hdr10_metadata(
        color_space,
        color_transfer,
        color_primaries,
        mastering_display,
        max_cll,
        is_hdr10_plus,
    ) && has_hdr10_sampling
}

/// Append HDR10-family x265 parameters when the source metadata is compatible with HDR10 signaling.
///
/// `hdr10=1` is emitted explicitly so HDR10+ remains signaled even when static HDR10 metadata is
/// absent. `master-display` / `max-cll` are intentionally suppressed for non-PQ paths because x265
/// treats them as HDR10 signaling knobs, which can mislabel HLG or BT.2020 SDR output.
pub fn append_x265_hdr10_params(
    params: &mut String,
    color_space: Option<&str>,
    color_transfer: Option<&str>,
    color_primaries: Option<&str>,
    mastering_display: Option<&str>,
    max_cll: Option<&str>,
    is_hdr10_plus: bool,
    output_pix_fmt: &str,
    hdr10plus_json: Option<&Path>,
) {
    let should_emit = should_emit_x265_hdr10_metadata(
        color_space,
        color_transfer,
        color_primaries,
        mastering_display,
        max_cll,
        is_hdr10_plus,
    );
    if !should_emit {
        return;
    }

    crate::x265_params::push_param_if_missing(params, "hdr10=1");

    if should_enable_x265_hdr10_opt(
        color_space,
        color_transfer,
        color_primaries,
        mastering_display,
        max_cll,
        is_hdr10_plus,
        output_pix_fmt,
    ) {
        crate::x265_params::push_param_if_missing(params, "hdr-opt=1");
        crate::x265_params::push_param_if_missing(params, "repeat-headers=1");
    }

    if let Some(mastering_display) = mastering_display.filter(|value| !value.trim().is_empty()) {
        crate::x265_params::push_param_if_missing(
            params,
            &format!("master-display={mastering_display}"),
        );
    }
    if let Some(max_cll) = max_cll.filter(|value| !value.trim().is_empty()) {
        crate::x265_params::push_param_if_missing(params, &format!("max-cll={max_cll}"));
    }
    if let Some(hdr10plus_json) = hdr10plus_json {
        crate::x265_params::push_param_if_missing(
            params,
            &format!("dhdr10-info={}", hdr10plus_json.display()),
        );
    }
}

/// Merge pre-built x265 params with probe-derived HDR10 metadata once per explore session.
///
/// Used at GPU explore init so per-encode `inject_hdr_metadata` does not treat an empty base as a
/// fallback path.
#[must_use]
pub fn merge_hevc_x265_params_from_probe(
    base: Option<&str>,
    probe: &crate::ffprobe::FFprobeResult,
) -> Option<String> {
    let mut updated = crate::media_conversion_gate::x265_params_base_owned_or_empty(base);
    let has_hdr10plus_signal = probe.hdr.hdr10_plus
        || crate::x265_params::has_hdr10plus_metadata(Some(updated.as_str()))
        || crate::x265_params::has_hdr10plus_metadata(base);
    append_x265_hdr10_params(
        &mut updated,
        probe.color_space.as_deref(),
        probe.color_transfer.as_deref(),
        probe.color_primaries.as_deref(),
        probe.hdr.mastering_display.as_deref(),
        probe.hdr.max_cll.as_deref(),
        has_hdr10plus_signal,
        crate::media_precision::hevc_yuv420_output_pix_fmt(probe),
        None,
    );
    if updated.is_empty() {
        None
    } else {
        Some(updated)
    }
}

#[must_use]
pub fn infer_bt709_if_modern(mut info: ColorInfo, width: u32, height: u32, ext: &str) -> ColorInfo {
    let is_hd = width >= 1280 || height >= 720;
    let is_modern_format = matches!(
        ext.to_lowercase().as_str(),
        "avif" | "webp" | "jxl" | "heic" | "heif" | "apng"
    );
    let assessment = info.assessment();
    if assessment.has_hdr_signaling() || assessment.has_confirmed_high_bit_depth() {
        return info;
    }
    if is_hd || is_modern_format {
        let mut inferred = Vec::new();
        if info.color_space.is_none() {
            info.color_space = Some("bt709".to_string());
            inferred.push("color_space");
        }
        if info.color_transfer.is_none() {
            info.color_transfer = Some("iec61966-2-1".to_string());
            inferred.push("color_transfer");
        }
        if info.color_primaries.is_none() {
            info.color_primaries = Some("bt709".to_string());
            inferred.push("color_primaries");
        }
        if !inferred.is_empty() {
            crate::media_conversion_gate::delivery_metadata_batch_audit(
                "hdr_bt709_cicp_inference",
                format!(
                    "{ext} {width}x{height}: missing CICP tags; inferred {} for encode (not measured ICC)",
                    inferred.join(", ")
                ),
            );
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
    fn test_build_yuv_output_ffmpeg_color_args_normalizes_and_skips_rgb() {
        assert_eq!(
            build_yuv_output_ffmpeg_color_args(
                Some("bt2020_ncl"),
                Some("smpte2084"),
                Some("bt2020"),
            ),
            vec![
                "-colorspace",
                "bt2020nc",
                "-color_trc",
                "smpte2084",
                "-color_primaries",
                "bt2020"
            ]
        );

        assert_eq!(
            build_yuv_output_ffmpeg_color_args(Some("rgb"), Some("bt709"), Some("bt709")),
            vec!["-color_trc", "bt709", "-color_primaries", "bt709"]
        );
    }

    #[test]
    fn test_color_info_to_ffmpeg_args_skips_unknown_and_preserves_range() {
        let info = ColorInfo {
            color_primaries: Some(crate::constants::STR_UNKNOWN.to_string()),
            color_transfer: Some("bt709".to_string()),
            color_space: Some(String::new()),
            color_range: Some("pc".to_string()),
            ..Default::default()
        };

        assert_eq!(
            color_info_to_ffmpeg_args(&info),
            vec!["-color_trc", "bt709", "-color_range", "pc"]
        );
    }

    #[test]
    fn test_color_info_needs_high_precision_png_decode() {
        let hdr_info = ColorInfo {
            color_transfer: Some("smpte2084".to_string()),
            bit_depth: Some(10),
            ..Default::default()
        };
        assert!(hdr_info.needs_high_precision_png_decode());

        let sdr_info = ColorInfo {
            bit_depth: Some(8),
            ..Default::default()
        };
        assert!(!sdr_info.needs_high_precision_png_decode());

        let inferred_high_precision = ColorInfo {
            bit_depth: Some(10),
            bit_depth_inferred_from_pix_fmt: true,
            ..Default::default()
        };
        assert!(inferred_high_precision.needs_high_precision_png_decode());
    }

    #[test]
    fn test_color_info_png_decode_rgb_pix_fmt() {
        let hdr_info = ColorInfo {
            bit_depth: Some(10),
            ..Default::default()
        };
        assert_eq!(
            hdr_info.png_decode_rgb_pix_fmt(),
            crate::constants::PIX_FMT_RGB48LE
        );

        let sdr_info = ColorInfo {
            bit_depth: Some(8),
            ..Default::default()
        };
        assert_eq!(
            sdr_info.png_decode_rgb_pix_fmt(),
            crate::constants::PIX_FMT_RGB24
        );

        let float_info = ColorInfo {
            is_float: true,
            bit_depth: Some(32),
            ..Default::default()
        };
        assert!(!float_info.needs_high_precision_png_decode());
        assert_eq!(
            float_info.png_decode_rgb_pix_fmt(),
            crate::constants::PIX_FMT_RGB24
        );
    }

    #[test]
    fn test_should_enable_x265_hdr10_opt_for_hdr10_triplet() {
        assert!(should_enable_x265_hdr10_opt(
            Some("bt2020nc"),
            Some("smpte2084"),
            Some("bt2020"),
            Some("G(8500,39850)B(6550,2300)R(35400,14600)WP(15635,16450)L(10000000,1)"),
            Some("1000,400"),
            false,
            "yuv420p10le",
        ));
    }

    #[test]
    fn test_should_emit_x265_hdr10_metadata_rejects_hlg_static_metadata() {
        assert!(!should_emit_x265_hdr10_metadata(
            Some("bt2020nc"),
            Some(crate::constants::HDR_TRANSFER_HLG),
            Some("bt2020"),
            Some("G(8500,39850)B(6550,2300)R(35400,14600)WP(15635,16450)L(10000000,1)"),
            Some("1000,400"),
            false,
        ));
    }

    #[test]
    fn test_should_enable_x265_hdr10_opt_rejects_hlg() {
        assert!(!should_enable_x265_hdr10_opt(
            Some("bt2020nc"),
            Some(crate::constants::HDR_TRANSFER_HLG),
            Some("bt2020"),
            None,
            None,
            false,
            "yuv420p10le",
        ));
    }

    #[test]
    fn test_color_info_to_x265_hdr_params_rejects_hlg() {
        let info = ColorInfo {
            color_primaries: Some("bt2020".to_string()),
            color_transfer: Some(crate::constants::HDR_TRANSFER_HLG.to_string()),
            color_space: Some("bt2020nc".to_string()),
            mastering_display: Some(
                "G(8500,39850)B(6550,2300)R(35400,14600)WP(15635,16450)L(10000000,1)".to_string(),
            ),
            max_cll: Some("1000,400".to_string()),
            ..Default::default()
        };

        assert_eq!(color_info_to_x265_hdr_params(&info), None);
    }

    #[test]
    fn test_color_info_to_x265_hdr_params_includes_hdr10_flags_for_hdr10plus() {
        let info = ColorInfo {
            color_primaries: Some("bt2020".to_string()),
            color_transfer: Some(crate::constants::HDR_TRANSFER_PQ.to_string()),
            color_space: Some("bt2020nc".to_string()),
            is_hdr10_plus: true,
            ..Default::default()
        };

        assert_eq!(
            color_info_to_x265_hdr_params(&info),
            Some(
                "colorprim=9:transfer=16:colormatrix=9:hdr10=1:hdr-opt=1:repeat-headers=1"
                    .to_string()
            )
        );
    }

    #[test]
    fn test_should_enable_x265_hdr10_opt_rejects_bt2020_sdr() {
        assert!(!should_enable_x265_hdr10_opt(
            Some("bt2020nc"),
            Some("bt709"),
            Some("bt2020"),
            None,
            None,
            false,
            "yuv420p10le",
        ));
    }

    #[test]
    fn test_append_x265_hdr10_params_for_hdr10plus_without_static_metadata_sets_hdr10() {
        let mut params = String::new();
        append_x265_hdr10_params(
            &mut params,
            Some("bt2020nc"),
            Some(crate::constants::HDR_TRANSFER_PQ),
            Some("bt2020"),
            None,
            None,
            true,
            "yuv420p10le",
            Some(std::path::Path::new("/tmp/hdr10plus.json")),
        );

        assert_eq!(
            params,
            "hdr10=1:hdr-opt=1:repeat-headers=1:dhdr10-info=/tmp/hdr10plus.json"
        );
    }

    #[test]
    fn test_append_x265_hdr10_params_rejects_hlg_even_with_static_metadata() {
        let mut params = String::new();
        append_x265_hdr10_params(
            &mut params,
            Some("bt2020nc"),
            Some(crate::constants::HDR_TRANSFER_HLG),
            Some("bt2020"),
            Some("G(8500,39850)B(6550,2300)R(35400,14600)WP(15635,16450)L(10000000,1)"),
            Some("1000,400"),
            false,
            "yuv420p10le",
            None,
        );

        assert!(params.is_empty());
    }

    #[test]
    fn test_infer_bt709_if_modern_allows_pix_fmt_inferred_high_bit_depth() {
        let inferred = ColorInfo {
            bit_depth: Some(10),
            bit_depth_inferred_from_pix_fmt: true,
            ..Default::default()
        };

        let enriched = infer_bt709_if_modern(inferred, 1920, 1080, "avif");
        assert_eq!(enriched.color_space.as_deref(), Some("bt709"));
        assert_eq!(enriched.color_transfer.as_deref(), Some("iec61966-2-1"));
        assert_eq!(enriched.color_primaries.as_deref(), Some("bt709"));
    }

    #[test]
    fn test_hdr_synthesis_math() {
        use image::{ImageBuffer, Luma, Rgb};

        // 1. Setup simple 2x2 SDR image (all mid-gray)
        let sdr_buf: ImageBuffer<Rgb<u8>, Vec<u8>> =
            ImageBuffer::from_pixel(2, 2, Rgb([128, 128, 128]));
        let sdr = DynamicImage::ImageRgb8(sdr_buf);

        // 2. Setup 2x2 Gainmap image (all mid-gray = log2(gain_max)/2 gain)
        let gain_buf: ImageBuffer<Luma<u8>, Vec<u8>> = ImageBuffer::from_pixel(2, 2, Luma([128]));
        let gain = DynamicImage::ImageLuma8(gain_buf);

        // 3. Setup params (2.0 gain max = 4x linear gain)
        let params = GainMapParams {
            gain_map_max: 2.0,
            gain_map_min: 0.0,
            gamma: 1.0,
            ..Default::default()
        };

        // 4. Run synthesis
        let result = synthesize(&sdr, &gain, &params, false).expect("Synthesis failed");

        assert_eq!(result.len(), 2 * 2 * 3);
        // Mid-gray (128/255) sRGB -> Linear is ~0.215
        // Gainmap 128/255 -> 0.5 * (2.0 - 0.0) + 0.0 = 1.0 (log2) -> 2.0x gain
        // Result should be approximately (0.215 + 0.01) * 2.0 - 0.01 = 0.44
        assert!(
            result[0] > 0.4 && result[0] < 0.5,
            "Expected ~0.44, got {}",
            result[0]
        );
    }

    #[test]
    fn parse_gainmap_from_xmp_malformed_numeric_returns_error() {
        let err = parse_gainmap_from_xmp(br#"<x:xmpmeta GainMapMax="not-a-number"/>"#)
            .expect_err("malformed gainmap numeric metadata must be an error");

        assert!(err.to_string().contains("not-a-number"));
    }

    #[test]
    fn classify_hdr_auxiliary_type_accepts_lowercase_gainmap_and_depth() {
        assert_eq!(
            classify_hdr_auxiliary_type("urn:com:apple:heif:auxiliary:gainmap"),
            HdrAuxiliaryRole::GainMap
        );
        assert_eq!(
            classify_hdr_auxiliary_type("urn:mpeg:mpegB:iclp:AuxiliaryDepth"),
            HdrAuxiliaryRole::Depth
        );
        assert_eq!(
            classify_hdr_auxiliary_type("urn:example:auxiliary:alpha"),
            HdrAuxiliaryRole::Other
        );
    }

    #[test]
    fn test_sidecar_output_path_appends_suffix_before_extension() {
        let output = Path::new("/tmp/photo.jxl");
        assert_eq!(
            sidecar_output_path(output, "gainmap", "jpg"),
            PathBuf::from("/tmp/photo.gainmap.jpg")
        );
        assert_eq!(
            sidecar_output_path(output, "depth", "png"),
            PathBuf::from("/tmp/photo.depth.png")
        );
    }

    #[test]
    fn test_persist_hdr_artifacts_writes_expected_sidecar() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let output = temp_dir.path().join("frame.jxl");
        std::fs::write(&output, b"not-a-real-jxl")?;

        let mut artifacts = HdrArtifacts::default();
        artifacts.push_raw_sidecar(
            "gainmap",
            "jpg",
            vec![0xFF, 0xD8, 0xFF, 0xD9],
            "Raw UltraHDR gainmap JPEG sidecar",
        );

        let written = persist_hdr_artifacts(&output, &artifacts)?;
        assert_eq!(written.len(), 1);
        assert_eq!(written[0], temp_dir.path().join("frame.gainmap.jpg"));
        assert_eq!(std::fs::read(&written[0])?, vec![0xFF, 0xD8, 0xFF, 0xD9]);
        Ok(())
    }
}
