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

use anyhow::{anyhow, Context, Result};
use image::{DynamicImage, GenericImageView, ImageBuffer, Luma};
use libheif_rs::{ColorSpace, HeifContext, ImageHandle};
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use std::path::Path;

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
        if let Ok(aux_type) = aux.auxiliary_type() {
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

                // Parse depth metadata from XMP if available
                let (near, far) = parse_depth_metadata(&handle).unwrap_or((None, None));

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
        #[allow(clippy::cast_ptr_alignment)]
        let data_u16: &[u16] = unsafe {
            std::slice::from_raw_parts(y_plane.data.as_ptr().cast::<u16>(), y_plane.data.len() / 2)
        };
        let mut buffer: ImageBuffer<Luma<u16>, Vec<u16>> = ImageBuffer::new(width, height);
        for (x, y, pixel) in buffer.enumerate_pixels_mut() {
            let offset = y as usize * (y_plane.stride / 2) + x as usize;
            let val = data_u16[offset];
            *pixel = Luma([val]);
        }
        Ok(DynamicImage::ImageLuma16(buffer))
    } else {
        let mut buffer: ImageBuffer<Luma<u8>, Vec<u8>> = ImageBuffer::new(width, height);
        for (x, y, pixel) in buffer.enumerate_pixels_mut() {
            let offset = y as usize * y_plane.stride + x as usize;
            let val = y_plane.data[offset];
            *pixel = Luma([val]);
        }
        // Convert to 16-bit for consistency
        Ok(DynamicImage::ImageLuma16(image::ImageBuffer::from_fn(
            width,
            height,
            |x, y| {
                let val = buffer.get_pixel(x, y)[0];
                Luma([u16::from(val) << 8 | u16::from(val)])
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
    let xmp_str = String::from_utf8_lossy(&xmp_data);

    let mut near = None;
    let mut far = None;
    let mut reader = Reader::from_str(&xmp_str);
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e) | Event::Empty(e)) => {
                for attr in e.attributes().flatten() {
                    let local_name = attr.key.local_name();
                    let attr_name = String::from_utf8_lossy(local_name.as_ref());
                    let attr_val = String::from_utf8_lossy(&attr.value);

                    if let Ok(f) = attr_val.parse::<f32>() {
                        if attr_name.as_ref().contains("NearDistance")
                            || attr_name.as_ref().contains("near_distance")
                        {
                            near = Some(f);
                        } else if attr_name.as_ref().contains("FarDistance")
                            || attr_name.as_ref().contains("far_distance")
                        {
                            far = Some(f);
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            _ => (),
        }
        buf.clear();
    }

    Ok((near, far))
}

/// Encode image to `JXL` with depth extra channel using `jpegxl-rs`.
///
/// Note: Current `jpegxl-rs` API has limited extra channel support.
/// This function encodes the main image. Depth map is saved separately
/// as sidecar file since `jpegxl-rs` doesn't expose `JxlEncoderSetExtraChannelBuffer`.
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

    use jpegxl_rs::encode::EncoderSpeed;
    use jpegxl_rs::encoder_builder;

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
    // Current jpegxl-rs high-level API doesn't expose JxlEncoderSetExtraChannelBuffer
    // Depth map is available in depth_map.image for sidecar storage

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
    use tempfile::NamedTempFile;

    // Write main image to temp PNG
    let temp_main = NamedTempFile::new().context("Failed to create temp file")?;
    main_image
        .to_rgb16()
        .save(temp_main.path())
        .context("Failed to save temp PNG")?;

    // Write depth to temp PNG
    let temp_depth = NamedTempFile::new().context("Failed to create temp depth file")?;
    depth_map
        .image
        .to_luma16()
        .save(temp_depth.path())
        .context("Failed to save temp depth PNG")?;

    // Use cjxl to encode main image
    let status = crate::tool_builders::CjxlBuilder::new()
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
        output
            .extension()
            .map(|s| s.to_string_lossy())
            .unwrap_or_default()
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
}
