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
use quick_xml::XmlVersion;
use serde::{Deserialize, Serialize};
use std::env;
use std::path::Path;
use tracing::{info, warn};

use crate::image_builders::ExiftoolBuilder;
use crate::image_jpeg_analysis::extract_gainmap_from_jpeg;
use crate::jxl_builder::CjxlBuilder;

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
            offset_sdr: 1.0 / 64.0,
            offset_hdr: 1.0 / 64.0,
            use_base_color_space: true,
            base_rendition_is_hdr: false,
        }
    }
}

/// Main entry point for converting a `HEIC` with Gainmap to an HDR `JXL` via intermediate format.
///
/// # Errors
///
/// Returns an error if:
/// - The file cannot be read or parsed.
/// - No gainmap is found in auxiliary images.
/// - `SDR` or Gainmap decoding fails.
/// - `HDR` synthesis math fails.
/// - Intermediate files cannot be written or the `cjxl` tool fails.
/// # Errors
///
/// Returns an error if the HEIC file cannot be read, gainmap is missing, or synthesis fails.
// Rationale: This function handles complex, sequential initialization or business logic where further fragmentation would hinder readability and maintainability.
#[allow(
    clippy::too_many_lines,
    reason = "Complex orchestration logic where fragmenting state into smaller helpers would decrease readability and increase cognitive overhead."
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
    tracing::debug!(
        input = ?input.file_name().unwrap_or_default(),
        ?intermediate_format,
        "Starting HEIC to HDR JXL synthesis"
    );
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

    // 5. Perform Synthesis
    tracing::debug!(
        input = ?input.file_name().unwrap_or_default(),
        ?params,
        needs_p3_conversion,
        "Performing HDR GainMap synthesis"
    );
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
            (tmp_png, 203.0 * params.gain_map_max.exp2())
        }
    };

    // 7. Invoke cjxl
    // 7. Invoke cjxl
    let mut builder = crate::tool_builders::CjxlBuilder::new();
    builder
        .input(&tmp_file)
        .output(output)
        .distance(actual_distance)
        .effort(actual_effort)
        .apple_compat(apple_compat)
        .arg("-x")
        .arg("color_space=RGB_D65_SRG_Rel_PeQ");

    if let Some(it) = resolve_intensity_target(intensity_target) {
        builder.intensity_target(crate::numeric_cast::f64_to_f32_lossy(f64::from(it)));
        info!("Applying intensity_target {} for HDR synthesis", it);
    } else {
        warn!("No valid intensity_target — proceeding without --intensity_target");
    }

    let status = builder
        .build()
        .status()
        .context("Failed to execute cjxl for HDR synthesis")?;

    if !status.success() {
        if tmp_file.exists() {
            std::fs::remove_file(&tmp_file).unwrap_or_else(|e| {
                tracing::warn!("Non-fatal cleanup/fallback operation failed: {}", e);
            });
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
        std::fs::remove_file(&tmp_file).unwrap_or_else(|e| {
            tracing::warn!("Non-fatal cleanup/fallback operation failed: {}", e);
        });
    }

    Ok(())
}

/// Main entry point for converting an `UltraHDR` JPEG to an HDR `JXL` via intermediate format.
///
/// `UltraHDR` JPEGs contain:
/// - Base `SDR` image (standard `JPEG`)
/// - Gainmap image (embedded via `MPF` - Multi Picture Format)
/// - `XMP` metadata with gainmap parameters (`hdrgm:` namespace)
///
/// # Errors
///
/// Returns an error if:
/// - The `JPEG` file cannot be read.
/// - Gainmap extraction from the `JPEG` fails.
/// - `HDR` synthesis math fails.
/// - Intermediate files cannot be written or the `cjxl` tool fails.
/// # Errors
///
/// Returns an error if the JPEG cannot be read, gainmap cannot be extracted, or synthesis fails.
pub fn convert_ultrahdr_jpeg_to_jxl_hdr(
    input: &Path,
    output: &Path,
    apple_compat: bool,
    intermediate_format: HdrIntermediateFormat,
    ultimate: bool,
) -> Result<()> {
    let actual_distance = crate::constants::jxl_distance_for_mode(1.0, ultimate);
    let actual_effort = crate::constants::jxl_effort_for_mode(ultimate);

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
    let params = parse_gainmap_params_from_jpeg_xmp(&data)
        .ok_or_else(|| anyhow::anyhow!("No valid XMP gainmap parameters found in the image"))?;
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
            (tmp_png, 203.0 * params.gain_map_max.exp2())
        }
    };

    // 7. Invoke cjxl
    let mut builder = crate::tool_builders::CjxlBuilder::new();
    builder
        .input(&tmp_file)
        .output(output)
        .distance(actual_distance)
        .effort(actual_effort)
        .apple_compat(apple_compat)
        .arg("-x")
        .arg("color_space=RGB_D65_SRG_Rel_PeQ");

    if let Some(it) = resolve_intensity_target(intensity_target) {
        builder.intensity_target(crate::numeric_cast::f64_to_f32_lossy(f64::from(it)));
        info!("Applying intensity_target {} for UltraHDR synthesis", it);
    } else {
        warn!("No valid intensity_target — proceeding without --intensity_target");
    }

    let status = builder
        .build()
        .status()
        .context("Failed to execute cjxl for HDR synthesis")?;

    if !status.success() {
        if tmp_file.exists() {
            std::fs::remove_file(&tmp_file).unwrap_or_else(|e| {
                tracing::warn!("Non-fatal cleanup/fallback operation failed: {}", e);
            });
        }
        return Err(anyhow!(
            "cjxl encoding failed with status {status} during UltraHDR JPEG HDR synthesis"
        ));
    }

    // 8. Cleanup
    if tmp_file.exists() {
        std::fs::remove_file(&tmp_file).unwrap_or_else(|e| {
            tracing::warn!("Non-fatal cleanup/fallback operation failed: {}", e);
        });
    }

    info!(
        "✅ UltraHDR JPEG HDR synthesis completed: {}",
        output.display()
    );
    Ok(())
}

/// Migration Path B: Encode `UltraHDR` JPEG to JXL with `GainMap` as sidecar.
///
/// This does NOT synthesize a single HDR plane. Instead, it:
/// 1. Extracts the SDR base image.
/// 2. Extracts the `GainMap` sub-image.
/// 3. Losslessly recompresses the SDR base to JXL.
/// 4. Saves the `GainMap` as a sidecar `.gainmap.png`.
/// 5. Preserves Ultra HDR XMP metadata (`hdrgm`) via `ExiftoolBuilder`.
///
/// This preserves the original SDR appearance bit-perfectly while
/// keeping the gainmap for future HDR reconstruction.
///
/// # Errors
///
/// Returns an error if extraction or encoding fails.
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

    info!(
        "📤 UltraHDR JPEG Gainmap migration started (Sidecar Path): {}",
        input.display()
    );

    // 1. Read and Extract
    let data = std::fs::read(input).context("Failed to read UltraHDR JPEG file")?;
    let (_base_image, gainmap_image) = extract_gainmap_from_jpeg(&data)
        .map_err(|e| anyhow!("Failed to extract gainmap for migration: {e}"))?;

    // 2. Save GainMap sidecar (.gainmap.png)
    let gainmap_sidecar = output.with_extension("gainmap.png");
    gainmap_image
        .save(&gainmap_sidecar)
        .context("Failed to save gainmap sidecar file")?;
    info!("💾 Gainmap saved as sidecar: {}", gainmap_sidecar.display());

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
        return Err(anyhow::anyhow!(
            "Lossless JPEG recompression failed for UltraHDR migration"
        ));
    }

    // 4. Preserve Ultra HDR Metadata via Exiftool
    // We specifically target XMP-hdrgm and MPF segments
    let mut exiftool = ExiftoolBuilder::new();
    exiftool
        .tags_from_file(input)
        .input(output)
        .overwrite_original()
        .preserve_date()
        .ignore_minor();

    let status = exiftool.build().status();
    if let Err(e) = status {
        warn!(
            "⚠️ Metadata preservation warning for {}: {}",
            output.display(),
            e
        );
    }

    info!("✅ UltraHDR JPEG migration completed: {}", output.display());
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
                let mut buffer = ImageBuffer::new(width, height);
                for (x, y, pixel) in buffer.enumerate_pixels_mut() {
                    let y_usize =
                        usize::try_from(y).map_err(|_| anyhow!("Y coordinate overflow: {y}"))?;
                    let x_usize =
                        usize::try_from(x).map_err(|_| anyhow!("X coordinate overflow: {x}"))?;
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
                    let y_usize =
                        usize::try_from(y).map_err(|_| anyhow!("Y coordinate overflow: {y}"))?;
                    let x_usize =
                        usize::try_from(x).map_err(|_| anyhow!("X coordinate overflow: {x}"))?;
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
                    let y_usize =
                        usize::try_from(y).map_err(|_| anyhow!("Y coordinate overflow: {y}"))?;
                    let x_usize =
                        usize::try_from(x).map_err(|_| anyhow!("X coordinate overflow: {x}"))?;
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
                    let y_usize =
                        usize::try_from(y).map_err(|_| anyhow!("Y coordinate overflow: {y}"))?;
                    let x_usize =
                        usize::try_from(x).map_err(|_| anyhow!("X coordinate overflow: {x}"))?;
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
    use crate::common_utils::find_box_data_recursive;

    // 1. Check colr/nclx box (Common in HEIC/AVIF/JXL containers)
    if let Some(colr_data) = find_box_data_recursive(data, *b"colr") {
        if colr_data.len() >= 11 && colr_data.get(0..4) == Some(b"nclx") {
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
    let slice = if let Some(s) = data.get(..end) {
        s
    } else {
        warn!("☢️ [ANOMALY] data.get(..{end}) failed for ICC search; using empty slice");
        &[]
    };

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
    parse_gainmap_from_xmp(&xmp_data)
}

// Rationale: This function handles complex, sequential initialization or business logic where further fragmentation would hinder readability and maintainability.
#[allow(
    clippy::too_many_lines,
    reason = "Complex orchestration logic where fragmenting state into smaller helpers would decrease readability and increase cognitive overhead."
)]
fn parse_gainmap_from_xmp(xmp_data: &[u8]) -> Option<GainMapParams> {
    let mut params = GainMapParams::default();
    let mut reader = Reader::from_reader(xmp_data);
    let mut buf = Vec::new();
    let mut found_any = false;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e) | Event::Empty(e)) => {
                // 1. Check Attributes (Common for Google/Samsung/ISO)
                for attr in e.attributes().flatten() {
                    let local_name = attr.key.local_name();

                    // Zero-copy attribute parsing
                    if let Ok(attr_val_cow) = attr.normalized_value(XmlVersion::Explicit1_0) {
                        if let Ok(f) = attr_val_cow.parse::<f32>() {
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
                }

                // 2. Check Child Elements (Common for Apple)
                let name_bytes = e.name();
                let name_ref = name_bytes.as_ref();

                if name_ref.windows(10).any(|w| w == b"GainMapMax") {
                    if let Ok(val) = reader.read_text(name_bytes) {
                        if let Ok(text_cow) = reader.decoder().decode(val.as_ref()) {
                            if let Ok(f) = text_cow.parse::<f32>() {
                                params.gain_map_max = f;
                                found_any = true;
                            }
                        }
                    }
                } else if name_ref.windows(10).any(|w| w == b"GainMapMin") {
                    if let Ok(val) = reader.read_text(name_bytes) {
                        if let Ok(text_cow) = reader.decoder().decode(val.as_ref()) {
                            if let Ok(f) = text_cow.parse::<f32>() {
                                params.gain_map_min = f;
                                found_any = true;
                            }
                        }
                    }
                } else if name_ref.windows(9).any(|w| w == b"OffsetSDR")
                    || name_ref.windows(9).any(|w| w == b"OffsetSdr")
                {
                    if let Ok(val) = reader.read_text(name_bytes) {
                        if let Ok(text_cow) = reader.decoder().decode(val.as_ref()) {
                            if let Ok(f) = text_cow.parse::<f32>() {
                                params.offset_sdr = f;
                                found_any = true;
                            }
                        }
                    }
                } else if name_ref.windows(9).any(|w| w == b"OffsetHDR")
                    || name_ref.windows(9).any(|w| w == b"OffsetHdr")
                {
                    if let Ok(val) = reader.read_text(name_bytes) {
                        if let Ok(text_cow) = reader.decoder().decode(val.as_ref()) {
                            if let Ok(f) = text_cow.parse::<f32>() {
                                params.offset_hdr = f;
                                found_any = true;
                            }
                        }
                    }
                } else if name_ref.windows(5).any(|w| w == b"Gamma") {
                    if let Ok(val) = reader.read_text(name_bytes) {
                        if let Ok(text_cow) = reader.decoder().decode(val.as_ref()) {
                            if let Ok(f) = text_cow.parse::<f32>() {
                                params.gamma = f;
                                found_any = true;
                            }
                        }
                    }
                }
            }
            Err(_) | Ok(Event::Eof) => break,
            _ => (),
        }
        buf.clear();
    }

    if found_any {
        Some(params)
    } else {
        None
    }
}

/// Performs the HDR synthesis calculation using the provided `GainMap`.
///
/// # Errors
///
/// Returns an error if the images have incompatible dimensions or if memory allocation fails.
// Rationale: This function handles complex, sequential initialization or business logic where further fragmentation would hinder readability and maintainability.
#[allow(
    clippy::too_many_lines,
    reason = "Complex orchestration logic where fragmenting state into smaller helpers would decrease readability and increase cognitive overhead."
)]
pub fn synthesize_hdr(
    sdr: &DynamicImage,
    gain: &DynamicImage,
    params: &GainMapParams,
    needs_p3_conversion: bool,
) -> Result<Vec<f32>> {
    use image::GenericImageView;
    let (width, height) = sdr.dimensions();

    let gain_resized_storage: DynamicImage;
    let gain_resized: &DynamicImage = if gain.dimensions() == (width, height) {
        gain
    } else {
        gain_resized_storage =
            gain.resize_exact(width, height, image::imageops::FilterType::Triangle);
        &gain_resized_storage
    };

    let total_pixels = (width * height * 3) as usize;
    let mut hdr_pixels = vec![0.0f32; total_pixels];

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
            let idx = ((y * width + x) * 3) as usize;
            // 1. Get Normalized SDR (Linearized later)
            let (r_norm, g_norm, b_norm) = sdr_16.as_ref().map_or_else(
                || {
                    sdr_8.as_ref().map_or_else(
                        || unreachable!("SDR buffer type mismatch"),
                        |buf| {
                            let p =
                                <image::ImageBuffer<image::Rgb<u8>, Vec<u8>>>::get_pixel(buf, x, y);
                            (
                                f32::from(p.0[0]) / 255.0,
                                f32::from(p.0[1]) / 255.0,
                                f32::from(p.0[2]) / 255.0,
                            )
                        },
                    )
                },
                |buf| {
                    let p = <image::ImageBuffer<image::Rgb<u16>, Vec<u16>>>::get_pixel(buf, x, y);
                    (
                        f32::from(p.0[0]) / 65535.0,
                        f32::from(p.0[1]) / 65535.0,
                        f32::from(p.0[2]) / 65535.0,
                    )
                },
            );

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
                gain_rgb16.as_ref().map_or_else(
                    || {
                        gain_rgb8.as_ref().map_or_else(
                            || {
                                let g_val = apply_gain(128.0, 255.0);
                                (g_val, g_val, g_val)
                            },
                            |buf| {
                                let p = <image::ImageBuffer<image::Rgb<u8>, Vec<u8>>>::get_pixel(
                                    buf, x, y,
                                );
                                (
                                    apply_gain(f32::from(p.0[0]), 255.0),
                                    apply_gain(f32::from(p.0[1]), 255.0),
                                    apply_gain(f32::from(p.0[2]), 255.0),
                                )
                            },
                        )
                    },
                    |buf| {
                        let p =
                            <image::ImageBuffer<image::Rgb<u16>, Vec<u16>>>::get_pixel(buf, x, y);
                        (
                            apply_gain(f32::from(p.0[0]), 65535.0),
                            apply_gain(f32::from(p.0[1]), 65535.0),
                            apply_gain(f32::from(p.0[2]), 65535.0),
                        )
                    },
                )
            } else {
                let g_val = gain_16.as_ref().map_or_else(
                    || {
                        gain_8.as_ref().map_or_else(
                            || apply_gain(128.0, 255.0),
                            |buf| {
                                let p = <image::ImageBuffer<image::Luma<u8>, Vec<u8>>>::get_pixel(
                                    buf, x, y,
                                );
                                apply_gain(f32::from(p.0[0]), 255.0)
                            },
                        )
                    },
                    |buf| {
                        let p =
                            <image::ImageBuffer<image::Luma<u16>, Vec<u16>>>::get_pixel(buf, x, y);
                        apply_gain(f32::from(p.0[0]), 65535.0)
                    },
                );
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

                if let Some(r) = hdr_pixels.get_mut(idx) {
                    *r = r_srgb.max(0.0);
                }
                if let Some(g) = hdr_pixels.get_mut(idx + 1) {
                    *g = g_srgb.max(0.0);
                }
                if let Some(b) = hdr_pixels.get_mut(idx + 2) {
                    *b = b_srgb.max(0.0);
                }
            } else {
                if let Some(r) = hdr_pixels.get_mut(idx) {
                    *r = r_hdr.max(0.0);
                }
                if let Some(g) = hdr_pixels.get_mut(idx + 1) {
                    *g = g_hdr.max(0.0);
                }
                if let Some(b) = hdr_pixels.get_mut(idx + 2) {
                    *b = b_hdr.max(0.0);
                }
            }
        }
    }

    Ok(hdr_pixels)
}

/// Resolve and sanitize an intensity target for `cjxl`.
///
/// - Honor `MFB_JXL_INTENSITY_TARGET` if set (numeric, nits).
/// - Clamp derived values into a safe range [100, `1_000_000`].
/// - Return `None` when the derived value is invalid.
fn resolve_intensity_target(derived: f32) -> Option<u32> {
    // Env override takes precedence
    if let Ok(ov) = env::var("MFB_JXL_INTENSITY_TARGET") {
        match ov.parse::<f32>() {
            Ok(v) if v.is_finite() && v > 0.0 => {
                let clamped = v.clamp(100.0_f32, 1_000_000.0_f32);
                if (clamped - v).abs() > f32::EPSILON {
                    warn!("MFB_JXL_INTENSITY_TARGET value {v} clamped to {clamped}");
                }
                return Some(crate::numeric_cast::f32_to_u32_sat(clamped.round()));
            }
            _ => {
                warn!("Invalid MFB_JXL_INTENSITY_TARGET='{}' — ignoring", ov);
            }
        }
    }

    if !derived.is_finite() || derived <= 0.0 {
        warn!(
            "Derived intensity_target invalid: {} — skipping --intensity_target",
            derived
        );
        return None;
    }

    let clamped = derived.clamp(100.0_f32, 1_000_000.0_f32);
    if (clamped - derived).abs() > f32::EPSILON {
        warn!(
            "Derived intensity_target {} clamped to {}",
            derived, clamped
        );
    }
    Some(crate::numeric_cast::f32_to_u32_sat(clamped.round()))
}

fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_pq(linear: f32) -> f32 {
    let l = (linear * 203.0) / 10000.0;
    let l = l.clamp(0.0, 1.0);

    let m1 = 2610.0 / 16384.0;
    let m2 = 2523.0 / 32.0;
    let c1 = 3424.0 / 4096.0;
    let c2 = 2413.0 / 128.0;
    let c3 = 2392.0 / 128.0;

    let lm = l.powf(m1);
    let num = c1 + c2 * lm;
    let den = 1.0 + c3 * lm;
    (num / den).powf(m2)
}

/// Write HDR pixels to a 16-bit PNG file using the PQ (ST 2084) transfer curve.
fn write_png16(pixels: &[f32], width: u32, height: u32, path: &Path) -> Result<()> {
    use image::{ImageBuffer, Rgb};

    let mut buffer: ImageBuffer<Rgb<u16>, Vec<u16>> = ImageBuffer::new(width, height);

    for (x, y, pixel) in buffer.enumerate_pixels_mut() {
        let y_u64 = u64::from(y);
        let x_u64 = u64::from(x);
        let width_u64 = u64::from(width);
        let idx_u64 = y_u64
            .checked_mul(width_u64)
            .and_then(|v| v.checked_add(x_u64))
            .and_then(|v| v.checked_mul(3))
            .ok_or_else(|| anyhow!("Pixel index calculation overflow at ({x}, {y})"))?;
        let idx =
            usize::try_from(idx_u64).map_err(|_| anyhow!("Pixel index too large: {idx_u64}"))?;

        let r = crate::numeric_cast::f32_to_u16_sat(
            linear_to_pq(*pixels.get(idx).ok_or_else(|| {
                anyhow!("Pixel buffer too short: index {} >= {}", idx, pixels.len())
            })?) * 65535.0,
        );
        let g = crate::numeric_cast::f32_to_u16_sat(
            linear_to_pq(*pixels.get(idx + 1).ok_or_else(|| {
                anyhow!(
                    "Pixel buffer too short: index {} >= {}",
                    idx + 1,
                    pixels.len()
                )
            })?) * 65535.0,
        );
        let b = crate::numeric_cast::f32_to_u16_sat(
            linear_to_pq(*pixels.get(idx + 2).ok_or_else(|| {
                anyhow!(
                    "Pixel buffer too short: index {} >= {}",
                    idx + 2,
                    pixels.len()
                )
            })?) * 65535.0,
        );
        *pixel = Rgb([r, g, b]);
    }

    buffer
        .save(path)
        .context("Failed to save 16-bit PNG intermediate file")?;

    Ok(())
}

fn write_exr(pixels: &[f32], width: u32, height: u32, path: &Path) -> Result<()> {
    use exr::prelude::*;

    let width_usize = usize::try_from(width).map_err(|_| anyhow!("Width too large: {width}"))?;
    let height_usize =
        usize::try_from(height).map_err(|_| anyhow!("Height too large: {height}"))?;

    write_rgb_file(path, width_usize, height_usize, |x, y| {
        let idx = y
            .checked_mul(width_usize)
            .and_then(|v| v.checked_add(x))
            .and_then(|v| v.checked_mul(3))
            .unwrap_or(pixels.len()); // Out of bounds will be caught by get()
        (
            *pixels.get(idx).unwrap_or(&0.0),
            *pixels.get(idx + 1).unwrap_or(&0.0),
            *pixels.get(idx + 2).unwrap_or(&0.0),
        )
    })
    .context("Failed to write EXR file")?;

    Ok(())
}

/// Parse gainmap parameters from XMP data in a JPEG file.
fn parse_gainmap_params_from_jpeg_xmp(data: &[u8]) -> Option<GainMapParams> {
    use crate::image_jpeg_analysis::extract_xmp_from_jpeg_data;

    let xmp_blocks = extract_xmp_from_jpeg_data(data)?;
    for xmp_bytes in xmp_blocks {
        if let Some(params) = parse_gainmap_from_xmp(xmp_bytes.as_bytes()) {
            return Some(params);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::env;

    #[test]
    fn test_srgb_to_linear() {
        assert!(crate::float_compare::approx_eq_f64(
            f64::from(srgb_to_linear(0.0)),
            0.0
        ));
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
        let params = parse_gainmap_from_xmp(xmp.as_bytes())
            .unwrap_or_else(|| panic!("failed to parse gainmap"));
        assert!(crate::float_compare::approx_eq_f64(
            f64::from(params.gain_map_max),
            3.0
        ));
        assert!(crate::float_compare::approx_eq_f64(
            f64::from(params.gain_map_min),
            1.0
        ));
        assert!(crate::float_compare::approx_eq_f64(
            f64::from(params.offset_sdr),
            0.01
        ));
        assert!(crate::float_compare::approx_eq_f64(
            f64::from(params.offset_hdr),
            0.02
        ));
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
        let params = parse_gainmap_from_xmp(xmp.as_bytes())
            .unwrap_or_else(|| panic!("failed to parse gainmap"));
        assert!(crate::float_compare::approx_eq_f64(
            f64::from(params.gain_map_max),
            4.5
        ));
        assert!(crate::float_compare::approx_eq_f64(
            f64::from(params.gain_map_min),
            0.5
        ));
        assert!(crate::float_compare::approx_eq_f64(
            f64::from(params.gamma),
            2.2
        ));
        assert!(crate::float_compare::approx_eq_f64(
            f64::from(params.offset_sdr),
            0.05
        ));
        assert!(crate::float_compare::approx_eq_f64(
            f64::from(params.offset_hdr),
            0.08
        ));
    }

    #[test]
    #[serial]
    fn test_resolve_intensity_target_env_override() {
        let prev = env::var("MFB_JXL_INTENSITY_TARGET").ok();
        env::set_var("MFB_JXL_INTENSITY_TARGET", "5000");
        let got = resolve_intensity_target(100.0);
        assert_eq!(got, Some(5000));
        if let Some(v) = prev {
            env::set_var("MFB_JXL_INTENSITY_TARGET", v);
        } else {
            env::remove_var("MFB_JXL_INTENSITY_TARGET");
        }
    }

    #[test]
    #[serial]
    fn test_resolve_intensity_target_env_clamp() {
        let prev = env::var("MFB_JXL_INTENSITY_TARGET").ok();
        env::set_var("MFB_JXL_INTENSITY_TARGET", "2000000");
        let got = resolve_intensity_target(100.0);
        assert_eq!(got, Some(1_000_000));
        if let Some(v) = prev {
            env::set_var("MFB_JXL_INTENSITY_TARGET", v);
        } else {
            env::remove_var("MFB_JXL_INTENSITY_TARGET");
        }
    }

    #[test]
    fn test_resolve_intensity_target_derived_invalid() {
        // Clear environment variable to ensure clean test
        std::env::remove_var("MFB_JXL_INTENSITY_TARGET");

        // Negative derived value should be rejected
        let got = resolve_intensity_target(-1.0);
        assert_eq!(got, None);
    }

    #[test]
    fn test_resolve_intensity_target_derived_clamp() {
        // Very large derived value gets clamped
        let got = resolve_intensity_target(2_000_000.0);
        assert_eq!(got, Some(1_000_000));
    }
}
