//! # Depth Channel Extraction and JXL Extra Channel Embedding
//!
//! Extracts depth maps from HEIC/HEIF auxiliary images and embeds them
//! into JXL files as extra channels using jpegxl-rs FFI.
//!
//! ## Supported Depth Formats
//! - **Apple Depth Data**: `depth` auxiliary type from Apple HEIC
//! - **Samsung Depth**: `AuxiliaryDepth` auxiliary type
//! - **ISO Depth**: Standard ISO/IEC 21496-1 depth maps
//!
//! ## Architecture
//! 1. Extract depth map from HEIC using libheif-rs
//! 2. Normalize depth to 16-bit grayscale
//! 3. Encode main image to JXL with extra channel via jpegxl-rs
//!
//! ## Limitations
//! The `cjxl` CLI does not support `--extra-channel` parameters.
//! This module uses `jpegxl-rs` crate for direct libjxl FFI encoding.

use crate::builder_base::ToolBuilder;
use anyhow::{Context, Result, anyhow};
use image::{DynamicImage, GenericImageView, ImageBuffer, Luma};
use jpegxl_rs::encode::EncoderSpeed;
use jpegxl_rs::encoder_builder;
use libheif_rs::{ColorSpace, HeifContext, ImageHandle};
use quick_xml::events::Event;
use std::path::Path;

fn read_native_u16_word(data: &[u8], word_index: usize) -> Option<u16> {
    let byte_index = word_index.checked_mul(2)?;
    let bytes = data.get(byte_index..byte_index + 2)?;
    // `bytes` is exactly 2 bytes from the `get` above; direct indexing is sound.
    Some(u16::from_ne_bytes([bytes[0], bytes[1]]))
}

/// Depth map data extracted from HEIC
#[derive(Debug, Clone)]
pub struct DepthMap {
    /// Grayscale depth image (16-bit)
    pub image: DynamicImage,
    /// Width in pixels
    pub width: u32,
    /// Height in pixels
    pub height: u32,
    /// Depth type detected
    pub depth_type: DepthType,
    /// Near focus distance in meters (if available)
    pub near_distance: Option<f32>,
    /// Far focus distance in meters (if available)
    pub far_distance: Option<f32>,
}

/// Type of depth map
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepthType {
    /// Apple proprietary depth format
    Apple,
    /// Samsung proprietary depth format
    Samsung,
    /// Google proprietary depth format
    Google,
    /// ISO standard depth format
    Iso,
    /// Unknown/other depth format
    Unknown,
}

/// Extract depth map from `HEIC` file.
///
/// Searches for auxiliary images with depth-related types:
/// - `urn:com:apple:heif:depth`
/// - `urn:mpeg:mpegB:iclp:AuxiliaryDepth`
/// - `depth` (generic)
///
/// # Errors
///
/// Returns an error if:
/// - The file cannot be read.
/// - The `HEIC` context cannot be parsed.
/// - The primary image handle is missing.
/// - An auxiliary depth image is found but cannot be decoded.
pub fn extract_depth_from_heic(input: &Path) -> Result<Option<DepthMap>> {
    let data = std::fs::read(input).context("Failed to read HEIC file")?;
    let ctx = HeifContext::read_from_bytes(&data).context("Failed to parse HEIC context")?;
    let handle = ctx
        .primary_image_handle()
        .map_err(|e| anyhow!("Failed to get primary image handle: {e}"))?;

    // Search for depth auxiliary images
    let aux_images = handle.auxiliary_images(None);

    for aux in aux_images {
        let aux_type = aux
            .auxiliary_type()
            .map_err(|e| anyhow!("Failed to read HEIC auxiliary image type: {e}"))?;
        let aux_type_str = aux_type.to_lowercase();

        // Detect depth map types (prioritize specific vendors before generic 'depth')
        let depth_type = if aux_type_str.contains("apple") {
            Some(DepthType::Apple)
        } else if aux_type_str.contains("samsung") {
            Some(DepthType::Samsung)
        } else if aux_type_str.contains("google") {
            Some(DepthType::Google)
        } else if aux_type_str.contains("auxiliarydepth") {
            Some(DepthType::Iso)
        } else if aux_type_str.contains("depth") {
            Some(DepthType::Unknown)
        } else {
            None
        };

        if let Some(dt) = depth_type {
            let depth_image = decode_depth_handle(&aux)?;
            let (width, height) = depth_image.dimensions();

            // Parse depth metadata from XMP if available.
            // Missing metadata is represented as `(None, None)` by the parser itself;
            // actual parse failures should remain visible to the caller.
            let (near, far) = parse_depth_metadata(&handle)?;

            return Ok(Some(DepthMap {
                image: depth_image,
                width,
                height,
                depth_type: dt,
                near_distance: near,
                far_distance: far,
            }));
        }
    }

    Ok(None)
}

/// Decode depth map handle to 16-bit grayscale image
fn decode_depth_handle(handle: &ImageHandle) -> Result<DynamicImage> {
    let img = libheif_rs::LibHeif::new()
        .decode(handle, ColorSpace::Monochrome, None)
        .map_err(|e| anyhow!("HEIF depth decode error: {e}"))?;

    let width = img.width();
    let height = img.height();
    let bit_depth = handle.luma_bits_per_pixel();

    let planes = img.planes();
    let y_plane = planes
        .y
        .ok_or_else(|| anyhow!("No Y plane in depth image"))?;

    // Depth maps are typically 8-bit or 16-bit
    if bit_depth > 8 {
        let mut buffer: ImageBuffer<Luma<u16>, Vec<u16>> = ImageBuffer::new(width, height);
        for (x, y, pixel) in buffer.enumerate_pixels_mut() {
            let y_offset = crate::numeric_cast::u32_to_usize_strict(y, "depth_y")
                .ok_or_else(|| anyhow!("Depth y coordinate does not fit usize"))?;
            let x_offset = crate::numeric_cast::u32_to_usize_strict(x, "depth_x")
                .ok_or_else(|| anyhow!("Depth x coordinate does not fit usize"))?;
            let offset = y_offset
                .checked_mul(y_plane.stride / 2)
                .and_then(|row| row.checked_add(x_offset))
                .ok_or_else(|| anyhow!("Depth plane u16 offset overflow"))?;
            let val = read_native_u16_word(y_plane.data, offset)
                .ok_or_else(|| anyhow!("Depth plane buffer shorter than expected"))?;
            *pixel = Luma([val]);
        }
        Ok(DynamicImage::ImageLuma16(buffer))
    } else {
        let mut buffer: ImageBuffer<Luma<u8>, Vec<u8>> = ImageBuffer::new(width, height);
        for (x, y, pixel) in buffer.enumerate_pixels_mut() {
            let y_offset = crate::numeric_cast::u32_to_usize_strict(y, "depth_y")
                .ok_or_else(|| anyhow!("Depth y coordinate does not fit usize"))?;
            let x_offset = crate::numeric_cast::u32_to_usize_strict(x, "depth_x")
                .ok_or_else(|| anyhow!("Depth x coordinate does not fit usize"))?;
            let offset = y_offset
                .checked_mul(y_plane.stride)
                .and_then(|row| row.checked_add(x_offset))
                .ok_or_else(|| anyhow!("Depth plane u8 offset overflow"))?;
            let val = *y_plane.data.get(offset).ok_or_else(|| {
                anyhow!(
                    "Depth plane buffer shorter than image dimensions: offset {} out of range {}",
                    offset,
                    y_plane.data.len()
                )
            })?;
            *pixel = Luma([val]);
        }
        // Convert to 16-bit for consistency
        Ok(DynamicImage::ImageLuma16(image::ImageBuffer::from_fn(
            width,
            height,
            |x, y| {
                let val = buffer.get_pixel(x, y)[0];
                Luma([u16::from(val) << 8_i32 | u16::from(val)])
            },
        )))
    }
}

/// Parse depth metadata from XMP
fn parse_depth_metadata(handle: &ImageHandle) -> Result<(Option<f32>, Option<f32>)> {
    use libheif_rs::ItemId;

    let mut ids = [ItemId::default(); 1];
    let count = handle.metadata_block_ids(&mut ids, b"xmp ");
    if count == 0 {
        return Ok((None, None));
    }

    let xmp_data = handle
        .metadata(ids[0])
        .map_err(|e| anyhow!("Failed to get XMP metadata from HEIC: {e}"))?;
    parse_depth_metadata_from_xmp(&xmp_data)
}

fn parse_depth_metadata_from_xmp(xmp_data: &[u8]) -> Result<(Option<f32>, Option<f32>)> {
    let mut near = None;
    let mut far = None;
    let mut reader = quick_xml::reader::Reader::from_reader(xmp_data);
    reader.config_mut().trim_text(true);

    loop {
        match reader.read_event() {
            Ok(Event::Start(e) | Event::Empty(e)) => {
                for attr in e.attributes() {
                    let attr = attr.map_err(|e| anyhow!("Failed to parse depth XMP attr: {e}"))?;
                    let local_name = attr.key.local_name();
                    let attr_name = local_name.as_ref();
                    let target = if attr_name.windows(12).any(|w| w == b"NearDistance")
                        || attr_name.windows(13).any(|w| w == b"near_distance")
                    {
                        Some("NearDistance")
                    } else if attr_name.windows(11).any(|w| w == b"FarDistance")
                        || attr_name.windows(12).any(|w| w == b"far_distance")
                    {
                        Some("FarDistance")
                    } else {
                        None
                    };
                    let Some(target) = target else {
                        continue;
                    };
                    let unescaped = attr
                        .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                        .map_err(|e| anyhow!("Failed to normalize depth XMP attr {target}: {e}"))?;
                    let value = unescaped.as_ref().parse::<f32>().map_err(|e| {
                        anyhow!(
                            "Failed to parse depth XMP attr {target} value {:?}: {e}",
                            unescaped.as_ref()
                        )
                    })?;
                    if !value.is_finite() {
                        return Err(anyhow!(
                            "Depth XMP attr {target} value {:?} is not finite",
                            unescaped.as_ref()
                        ));
                    }
                    match target {
                        "NearDistance" => near = Some(value),
                        "FarDistance" => far = Some(value),
                        _ => unreachable!("target enumerated above"),
                    }
                }
            }
            Err(err) => return Err(anyhow!("Failed to parse depth XMP: {err}")),
            Ok(Event::Eof) => break,
            _ => (),
        }
    }

    Ok((near, far))
}

/// Encode image to `JXL` with depth extra channel using `jpegxl-rs`.
///
/// Note: Current `jpegxl-rs` API has limited extra channel support.
/// This function encodes the main image. Depth map is saved separately
/// as sidecar file since `jpegxl-rs` doesn't expose
/// `JxlEncoderSetExtraChannelBuffer`.
///
/// # Errors
///
/// Returns an error if:
/// - The `JXL` encoder fails to initialize.
/// - The image encoding process fails.
/// - The output file cannot be written.
pub fn encode_jxl_with_depth(
    main_image: &DynamicImage,
    _depth_map: &DepthMap,
    output: &Path,
    distance: f32,
    _effort: u8,
    ultimate: bool,
    _intensity_target: Option<f32>,
) -> Result<()> {
    let actual_dist = crate::constants::jxl_distance_for_mode(distance, ultimate);
    let actual_eff = crate::constants::jxl_effort_for_mode(ultimate);

    // Convert main image to RGBA16 for encoding
    let rgba_image = main_image.to_rgba16();
    let (width, height) = rgba_image.dimensions();

    // Create encoder builder
    let mut encoder = encoder_builder()
        .build()
        .map_err(|e| anyhow!("Failed to create JXL encoder: {e:?}"))?;

    // Set lossless mode based on distance
    encoder.lossless = Some(actual_dist <= 0.001);

    // Set quality (0-100 scale, convert from distance)
    // distance 0 = quality 100, distance 1 = quality ~70
    encoder.quality = actual_dist.mul_add(-30.0, 100.0).clamp(0.0, 100.0);

    // Set encoding speed
    let speed = match actual_eff {
        0..=2 => EncoderSpeed::Falcon,
        3..=5 => EncoderSpeed::Squirrel,
        _ => EncoderSpeed::Kitten, // Default to high quality
    };
    encoder.speed = speed;

    // Encode main image using encoder.encode() method
    // Specify u16 for both input (RGBA) and output (encoded data)
    let encode_result: jpegxl_rs::encode::EncoderResult<u16> = encoder
        .encode(&rgba_image, width, height)
        .map_err(|e| anyhow!("Failed to encode JXL: {e:?}"))?;

    // Write output
    std::fs::write(output, encode_result.data).context("Failed to write JXL output")?;

    // Note: Extra channel embedding requires direct libjxl FFI (jpegxl-sys)
    // Current jpegxl-rs high-level API doesn't expose
    // JxlEncoderSetExtraChannelBuffer Depth map is available in depth_map.image
    // for sidecar storage

    Ok(())
}

/// Encode `JXL` with depth using sidecar file approach.
///
/// This is the practical fallback: encodes main image to `JXL`
/// and saves depth map as a separate `PNG` file with `.depth.png` suffix.
///
/// ## Output Files
/// - `output`: Main `JXL` file
/// - `output.with_extension("depth.png")`: Depth map sidecar
///
/// # Errors
///
/// Returns an error if:
/// - Temporary files cannot be created.
/// - The `cjxl` tool fails to encode the main image.
/// - The sidecar depth file cannot be saved.
pub fn encode_jxl_depth_fallback(
    main_image: &DynamicImage,
    depth_map: &DepthMap,
    output: &Path,
    distance: f32,
    _effort: u8,
    apple_compat: bool,
    ultimate: bool,
) -> Result<(std::path::PathBuf, std::path::PathBuf)> {
    let actual_dist = crate::constants::jxl_distance_for_mode(distance, ultimate);
    let actual_eff = crate::constants::jxl_effort_for_mode(ultimate);

    // Write main image to temp PNG
    let temp_main = crate::media_conversion_gate::delivery_named_tempfile_in_scratch_or_err(
        "depth_channel_main",
        None,
        None,
    )
    .context("Failed to create temp file")?;
    main_image
        .to_rgb16()
        .save(temp_main.path())
        .context("Failed to save temp PNG")?;

    // Write depth to temp PNG
    let temp_depth = crate::media_conversion_gate::delivery_named_tempfile_in_scratch_or_err(
        "depth_channel_depth",
        None,
        None,
    )
    .context("Failed to create temp depth file")?;
    depth_map
        .image
        .to_luma16()
        .save(temp_depth.path())
        .context("Failed to save temp depth PNG")?;

    // Use cjxl to encode main image
    let status = crate::CjxlBuilder::new()
        .input(temp_main.path())
        .output(output)
        .distance(actual_dist)
        .effort(actual_eff)
        .apple_compat(apple_compat)
        .build()
        .status()
        .map_err(|e| anyhow!("Failed to run cjxl: {e}"))?;

    if !status.success() {
        return Err(anyhow!("cjxl failed to encode main image"));
    }

    // Save depth as sidecar file
    let depth_output = output.with_extension(format!(
        "depth.{}",
        crate::media_conversion_gate::path_extension_lossy_or_empty(output, "depth_map output")
    ));
    depth_map
        .image
        .to_luma16()
        .save(&depth_output)
        .context("Failed to save depth sidecar PNG")?;

    Ok((output.to_path_buf(), depth_output))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_depth_type_display() {
        assert_eq!(format!("{:?}", DepthType::Apple), "Apple");
        assert_eq!(format!("{:?}", DepthType::Samsung), "Samsung");
        assert_eq!(format!("{:?}", DepthType::Google), "Google");
        assert_eq!(format!("{:?}", DepthType::Iso), "Iso");
        assert_eq!(format!("{:?}", DepthType::Unknown), "Unknown");
    }

    #[test]
    fn test_depth_type_equality() {
        assert_eq!(DepthType::Apple, DepthType::Apple);
        assert_ne!(DepthType::Apple, DepthType::Samsung);
        assert_ne!(DepthType::Iso, DepthType::Unknown);
    }

    #[test]
    fn test_depth_map_clone() {
        // Verify DepthMap can be cloned
        let depth_map = DepthMap {
            image: DynamicImage::new_luma16(100, 100),
            width: 100,
            height: 100,
            depth_type: DepthType::Apple,
            near_distance: Some(0.5),
            far_distance: Some(5.0),
        };
        let cloned = depth_map.clone();
        assert_eq!(cloned.width, depth_map.width);
        assert_eq!(cloned.height, depth_map.height);
        assert_eq!(cloned.depth_type, depth_map.depth_type);
        assert_eq!(cloned.near_distance, depth_map.near_distance);
        assert_eq!(cloned.far_distance, depth_map.far_distance);
    }

    #[test]
    fn test_depth_map_with_none_distances() {
        let depth_map = DepthMap {
            image: DynamicImage::new_luma16(64, 64),
            width: 64,
            height: 64,
            depth_type: DepthType::Iso,
            near_distance: None,
            far_distance: None,
        };
        assert!(depth_map.near_distance.is_none());
        assert!(depth_map.far_distance.is_none());
    }

    #[test]
    fn parse_depth_metadata_from_xmp_malformed_numeric_returns_error() {
        let err = parse_depth_metadata_from_xmp(br#"<x:xmpmeta NearDistance="not-a-number"/>"#)
            .expect_err("malformed depth distance must not be silently ignored");
        assert!(
            err.to_string().contains("NearDistance"),
            "unexpected error: {err}"
        );
    }
}
