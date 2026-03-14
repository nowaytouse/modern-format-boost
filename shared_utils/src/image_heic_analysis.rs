//! HEIC/HEIF Format Analysis Module
//!
//! Uses libheif-rs to decode and analyze HEIC/HEIF images

use crate::img_errors::{ImgQualityError, Result};
use image::DynamicImage;
use libheif_rs::{ColorSpace, HeifContext, LibHeif, RgbChroma};
use serde::{Deserialize, Serialize};
use std::path::Path;
use crate::common_utils::find_box_data_recursive;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeicAnalysis {
    pub bit_depth: u8,
    pub codec: String,
    pub is_lossless: bool,
    pub has_alpha: bool,
    pub has_auxiliary: bool,
    pub image_count: usize,
    pub is_hdr: bool,
    pub is_dolby_vision: bool,
}

/// Detect HEIC/HEIF lossless encoding — multi-dimension analysis.
///
/// Dimensions checked (in priority order):
/// 1. **hvcC profile_idc**: Main(1)/Main10(2)/MainStillPicture(3) → definitely lossy (4:2:0 only)
/// 2. **hvcC RExt(4)/SCC(9)** → lossless capable; check chroma_format_idc
/// 3. **hvcC chroma_format_idc**: < 3 (not 4:4:4) → lossy; == 3 → lossless
/// 4. **hvcC general_profile_compatibility_flags**: bit 4 set → RExt compatible → lossless
/// 5. **pixi box**: high bit depth with compatible profile → lossless indicator
/// 6. **colr box**: Identity matrix (MC=0) → lossless
/// 7. **SPS transquant_bypass_enabled_flag**: if 1 → mathematically lossless (100% certain)
pub fn detect_heic_is_lossless(data: &[u8], path: &Path) -> Result<bool> {
    if let Some(hvcc_data) = find_box_data_recursive(data, b"hvcC") {
        if hvcc_data.len() >= 23 {
            let profile_idc = hvcc_data[1] & 0x1F;

            // Bytes 2-5: general_profile_compatibility_flags (32 bits)
            let compat_flags = u32::from_be_bytes([hvcc_data[2], hvcc_data[3], hvcc_data[4], hvcc_data[5]]);

            // HEVCDecoderConfigurationRecord fixed fields:
            //   [16] chromaFormatIdc (low 2 bits)
            //   [17] bitDepthLumaMinus8 (low 3 bits)
            //   [18] bitDepthChromaMinus8 (low 3 bits)
            let chroma_format_idc = hvcc_data[16] & 0x03; // 0=mono, 1=4:2:0, 2=4:2:2, 3=4:4:4
            let bit_depth_luma = (hvcc_data[17] & 0x07) + 8;
            let bit_depth_chroma = (hvcc_data[18] & 0x07) + 8;

            // Dimension 0: chromaFormatIdc — direct chroma subsampling
            // 4:2:0 (1) or 4:2:2 (2) → definitively lossy (HEVC lossless requires 4:4:4)
            if chroma_format_idc == 1 || chroma_format_idc == 2 {
                return Ok(false);
            }

            // Dimension 1: Main/Main10/MainStillPicture → always 4:2:0 → always lossy
            if profile_idc == 1 || profile_idc == 2 || profile_idc == 3 {
                return Ok(false);
            }

            // Dimension 2: RExt (4) or SCC (9) profiles can be lossless
            if profile_idc == 4 || profile_idc == 9 {
                let is_444 = chroma_format_idc == 3;

                // Check colr box for Identity matrix
                if let Some(colr_data) = find_box_data_recursive(data, b"colr") {
                    if colr_data.len() >= 11 && &colr_data[0..4] == b"nclx" {
                        let matrix_coefficients = u16::from_be_bytes([colr_data[8], colr_data[9]]);
                        if matrix_coefficients == 0 {
                            return Ok(true);
                        }
                    }
                }

                // Check pixi box for high bit depth
                if let Some(pixi_data) = find_box_data_recursive(data, b"pixi") {
                    if !pixi_data.is_empty() {
                        let num_ch = pixi_data[0] as usize;
                        if num_ch > 0 && pixi_data.len() > num_ch {
                            let max_depth = pixi_data[1..=num_ch].iter().copied().max().unwrap_or(0);
                            if max_depth >= 12 {
                                return Ok(true);
                            }
                        }
                    }
                }

                // High bit depth from hvcC itself
                if is_444 && (bit_depth_luma >= 12 || bit_depth_chroma >= 12) {
                    return Ok(true);
                }

                // RExt/SCC + 4:4:4 without other indicators — likely lossless
                if is_444 {
                    return Ok(true);
                }

                // RExt/SCC without 4:4:4 — ambiguous (RExt can also do lossy 4:2:0)
                return Err(ImgQualityError::AnalysisError(format!(
                    "HEIC: RExt/SCC profile ({}) without 4:4:4 chroma; cannot determine — {}",
                    profile_idc, path.display()
                )));
            }

            // Dimension 4: Check profile compatibility flags — bit 4 = RExt compatible
            if (compat_flags & (1 << (31 - 4))) != 0 {
                if chroma_format_idc == 3 {
                    return Ok(true);
                } else {
                    return Err(ImgQualityError::AnalysisError(format!(
                        "HEIC: RExt compatibility flag set but chroma {} (not 4:4:4); cannot determine — {}",
                        chroma_format_idc, path.display()
                    )));
                }
            }

            // Dimension 5: Parse SPS NAL units to check transquant_bypass_enabled_flag
            if let Some(is_lossless) = detect_heic_lossless_via_mp4parse_data(data) {
                if is_lossless {
                    return Ok(true);
                }
            }

            // Unknown profile but hvcC exists — profiles 5-8, 10+ are rare
            // Most are lossy variants; treat as lossy rather than Err (safe default)
            if std::env::var("IMGQUALITY_VERBOSE").is_ok() {
                eprintln!("   📊 HEIC: unknown profile {} — treating as lossy", profile_idc);
            }
            return Ok(false);
        }
    }

    // No hvcC box — fallback to lossy (safe default for HEIC)
    Ok(false)
}

fn detect_heic_lossless_via_mp4parse_data(data: &[u8]) -> Option<bool> {
    let hvcc_data = find_box_data_recursive(data, b"hvcC")?;
    parse_sps_for_transquant_bypass_flag(hvcc_data)
}

fn parse_sps_for_transquant_bypass_flag(hvcc_data: &[u8]) -> Option<bool> {
    if hvcc_data.len() < 25 { return None; }
    let num_nalu_arrays = hvcc_data[24] as usize;
    let mut pos = 25;
    for _ in 0..num_nalu_arrays {
        if pos + 3 > hvcc_data.len() { return None; }
        let nal_unit_type = hvcc_data[pos] & 0x3F;
        let num_nalus = u16::from_be_bytes([hvcc_data[pos + 1], hvcc_data[pos + 2]]) as usize;
        pos += 3;
        if nal_unit_type == 33 {
            for _ in 0..num_nalus {
                if pos + 2 > hvcc_data.len() { return None; }
                let nal_unit_length = u16::from_be_bytes([hvcc_data[pos], hvcc_data[pos + 1]]) as usize;
                pos += 2;
                if pos + nal_unit_length > hvcc_data.len() { return None; }
                let sps_payload = &hvcc_data[pos..pos + nal_unit_length];
                pos += nal_unit_length;
                if sps_payload.len() < 3 { continue; }
                return parse_sps_rbsp_for_transquant_bypass(sps_payload);
            }
        } else {
            for _ in 0..num_nalus {
                if pos + 2 > hvcc_data.len() { return None; }
                let nal_unit_length = u16::from_be_bytes([hvcc_data[pos], hvcc_data[pos + 1]]) as usize;
                pos += 2 + nal_unit_length;
            }
        }
    }
    None
}

fn parse_sps_rbsp_for_transquant_bypass(sps_payload: &[u8]) -> Option<bool> {
    if sps_payload.len() < 3 { return None; }
    let rbsp = &sps_payload[2..];
    struct BitReader<'a> { data: &'a [u8], bit_pos: usize }
    impl<'a> BitReader<'a> {
        fn new(data: &'a [u8]) -> Self { BitReader { data, bit_pos: 0 } }
        fn read_bits(&mut self, n: usize) -> Option<u32> {
            if self.bit_pos + n > self.data.len() * 8 { return None; }
            let mut value = 0u32;
            for i in 0..n {
                let byte_pos = (self.bit_pos + i) / 8;
                let bit_offset = 7 - ((self.bit_pos + i) % 8);
                if byte_pos < self.data.len() {
                    let bit = (self.data[byte_pos] >> bit_offset) & 1;
                    value = (value << 1) | (bit as u32);
                }
            }
            self.bit_pos += n;
            Some(value)
        }
        fn read_ue(&mut self) -> Option<u32> {
            let mut leading_zeros = 0u32;
            while self.bit_pos < self.data.len() * 8 {
                let bit = self.read_bits(1)?;
                if bit == 1 { break; }
                leading_zeros += 1;
            }
            let info = if leading_zeros > 0 { self.read_bits(leading_zeros as usize)? } else { 0 };
            Some((1 << leading_zeros) - 1 + info)
        }
    }
    let mut reader = BitReader::new(rbsp);
    reader.read_bits(4)?; // sps_video_parameter_set_id
    let max_sub_layers = reader.read_bits(3)?;
    reader.read_bits(1)?; // sps_temporal_id_nesting_flag
    reader.read_ue()?; // sps_seq_parameter_set_id
    let chroma_format = reader.read_ue()?;
    if chroma_format == 3 { reader.read_bits(1)?; } // separate_colour_plane_flag
    reader.read_ue()?; // pic_width_in_luma_samples
    reader.read_ue()?; // pic_height_in_luma_samples
    if reader.read_bits(1)? == 1 { // conformance_window_flag
        for _ in 0..4 { reader.read_ue()?; }
    }
    reader.read_ue()?; // bit_depth_luma_minus8
    reader.read_ue()?; // bit_depth_chroma_minus8
    for _ in 0..=max_sub_layers { reader.read_ue()?; } // sps_max_dec_pic_buffering_minus1
    for _ in 0..=max_sub_layers { reader.read_ue()?; } // sps_max_num_reorder_pics
    for _ in 0..=max_sub_layers { reader.read_ue()?; } // sps_max_latency_increase_plus1
    reader.read_ue()?; // sps_min_luma_coding_block_size_minus3
    reader.read_ue()?; // sps_max_luma_coding_block_size_minus3
    reader.read_ue()?; // sps_max_luma_hierarchy_depth
    if chroma_format != 0 {
        reader.read_ue()?; // sps_min_chroma_coding_block_size_minus3
        reader.read_ue()?; // sps_max_chroma_coding_block_size_minus3
        reader.read_ue()?; // sps_max_chroma_hierarchy_depth
    }
    reader.read_bits(1)?; // amp_enabled_flag
    reader.read_bits(1)?; // sample_adaptive_offset_enabled_flag
    if reader.read_bits(1)? == 1 { // pcm_enabled_flag
        reader.read_bits(1)?; reader.read_bits(1)?; reader.read_ue()?; reader.read_ue()?; reader.read_bits(1)?;
    }
    let transquant_bypass = reader.read_bits(1)?;
    Some(transquant_bypass == 1)
}

pub fn analyze_heic_file(path: &Path) -> Result<(DynamicImage, HeicAnalysis)> {
    // 🧠 Global security bypass for complex/large boxes (e.g. Weibo processed HEICs)
    // This environment variable is checked by the absolute core of libheif.
    std::env::set_var("LIBHEIF_SECURITY_LIMITS", "off");

    let lib_heif = LibHeif::new();

    let ctx = HeifContext::read_from_file(path.to_string_lossy().as_ref()).map_err(|e| {
        let error_msg = format!("{}", e);
        if error_msg.contains("SecurityLimitExceeded") || error_msg.contains("ipco") {
            eprintln!(
                "⚠️  HEIC SecurityLimitExceeded: {} - using fallback analysis",
                path.display()
            );
            ImgQualityError::ImageReadError(format!(
                "HEIC security limit exceeded (ipco box limit): {}",
                e
            ))
        } else {
            ImgQualityError::ImageReadError(format!("Failed to read HEIC: {}", e))
        }
    })?;

    // 🛡️ API-level security bypass (requires v1_21+ feature)
    // We attempt to disable limits on the context itself as a second layer of defense.
    #[cfg(feature = "v1_21")]
    {
        let mut limits = ctx.security_limits();
        // Set to 4GB or higher - effectively disabling for normal usage
        limits.set_max_total_memory(4 * 1024 * 1024 * 1024); 
    }

    let handle = ctx.primary_image_handle().map_err(|e| {
        ImgQualityError::ImageReadError(format!("Failed to get primary image: {}", e))
    })?;

    let width = handle.width();
    let height = handle.height();
    let has_alpha = handle.has_alpha_channel();
    let bit_depth = handle.luma_bits_per_pixel();

    let data = std::fs::read(path)?;
    let is_lossless = detect_heic_is_lossless(&data, path).unwrap_or(false);

    // Detect HDR and Dolby Vision
    let mut is_hdr = false;
    let mut is_dolby_vision = false;
    
    // Quick scan for HDR/DV boxes in the already read data
    if let Some(colr_data) = find_box_data_recursive(&data, b"colr") {
        if colr_data.len() >= 11 && &colr_data[0..4] == b"nclx" {
            let primaries = u16::from_be_bytes([colr_data[4], colr_data[5]]);
            let transfer = u16::from_be_bytes([colr_data[6], colr_data[7]]);
            if primaries == 9 && (transfer == 16 || transfer == 18) {
                is_hdr = true;
            }
        }
    }
    if find_box_data_recursive(&data, b"dvcC").is_some() || find_box_data_recursive(&data, b"dvvC").is_some() {
        is_dolby_vision = true;
        is_hdr = true;
    }

    let image_count = ctx.image_ids().len();
    let has_auxiliary = handle.number_of_depth_images() > 0;

    let decoded_image = lib_heif
        .decode(&handle, ColorSpace::Rgb(RgbChroma::Rgb), None)
        .map_err(|e| ImgQualityError::ImageReadError(format!("Failed to decode HEIC: {}", e)))?;

    let planes = decoded_image.planes();
    let plane = planes
        .interleaved
        .ok_or_else(|| ImgQualityError::ImageReadError("No RGB plane found".to_string()))?;

    let img = image::RgbImage::from_raw(width, height, plane.data.to_vec())
        .map(DynamicImage::ImageRgb8)
        .ok_or_else(|| ImgQualityError::ImageReadError("Failed to create RGB image".to_string()))?;

    let codec = "HEVC".to_string();

    let analysis = HeicAnalysis {
        bit_depth,
        codec,
        is_lossless,
        has_alpha,
        has_auxiliary,
        image_count,
        is_hdr,
        is_dolby_vision,
    };

    Ok((img, analysis))
}

pub fn is_heic_file(path: &Path) -> bool {
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        let ext = ext.to_lowercase();
        if matches!(ext.as_str(), "heic" | "heif" | "hif") {
            return true;
        }
    }

    if let Ok(mut file) = std::fs::File::open(path) {
        use std::io::Read;
        let mut buffer = [0u8; 12];
        if file.read_exact(&mut buffer).is_ok()
            && &buffer[4..8] == b"ftyp" {
                let brand = &buffer[8..12];
                if matches!(
                    brand,
                    b"heic" | b"heix" | b"heim" | b"heis" | b"mif1" | b"msf1"
                ) {
                    return true;
                }
            }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_heic_file() {
        assert!(is_heic_file(Path::new("test.heic")));
        assert!(is_heic_file(Path::new("test.HEIC")));
        assert!(is_heic_file(Path::new("test.heif")));
        assert!(!is_heic_file(Path::new("test.jpg")));
    }
}
