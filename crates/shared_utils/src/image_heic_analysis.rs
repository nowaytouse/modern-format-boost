//! HEIC/HEIF Format Analysis Module
//!
//! Uses libheif-rs to decode and analyze HEIC/HEIF images

use crate::common_utils::find_box_data_recursive;
use crate::img_errors::{ImgQualityError, Result};
use image::DynamicImage;
use libheif_rs::{ColorSpace, HeifContext, LibHeif, RgbChroma};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tracing::{debug, warn};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HeicHdrInfo {
    pub is_hdr: bool,
    pub is_dolby_vision: bool,
    pub has_gainmap: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HeicAuxInfo {
    pub has_auxiliary: bool,
    pub has_vendor_metadata: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeicAnalysis {
    pub bit_depth: Option<u8>,
    pub codec: String,
    pub is_lossless: bool,
    pub has_alpha: bool,
    pub image_count: usize,
    pub hdr: HeicHdrInfo,
    pub aux: HeicAuxInfo,
}

/// Detect HEIC/HEIF lossless encoding — multi-dimension analysis.
///
/// Dimensions checked (in priority order):
/// 1. **hvcC `profile_idc`**: Main(1)/Main10(2)/MainStillPicture(3) → definitely lossy (4:2:0 only)
/// 2. **hvcC RExt(4)/SCC(9)** → lossless capable; check `chroma_format_idc`
/// 3. **hvcC `chroma_format_idc`**: < 3 (not 4:4:4) → lossy; == 3 → lossless
/// 4. **hvcC `general_profile_compatibility_flags`**: bit 4 set → `RExt` compatible → lossless
/// 5. **pixi box**: high bit depth with compatible profile → lossless indicator
/// 6. **colr box**: Identity matrix (MC=0) → lossless
/// 7. **SPS `transquant_bypass_enabled_flag`**: if 1 → mathematically lossless (100% certain)
///
/// Detect if an HEIC file is lossless (using libheif).
///
/// # Errors
/// Returns an error if the file cannot be read or libheif fails.
// Rationale: This function handles complex, sequential initialization or business logic where further fragmentation would hinder readability and maintainability.
#[allow(
    clippy::too_many_lines,
    reason = "Complex orchestration logic where fragmenting state into smaller helpers would decrease readability and increase cognitive overhead."
)]
pub fn detect_heic_is_lossless(data: &[u8], path: &Path) -> Result<bool> {
    // Try find_box_data_recursive first, then fallback to direct magic byte search
    // This handles cases where boxes are inside full boxes (e.g. meta box with version/flags)
    let hvcc_from_recursive = find_box_data_recursive(data, *b"hvcC");
    let hvcc_from_magic = find_box_payload_by_magic(data, *b"hvcC");

    debug!(
        path = %path.display(),
        "Checking HEIC lossless status"
    );
    debug!(
        "   hvcc_from_recursive: {}",
        if hvcc_from_recursive.is_some() {
            "found"
        } else {
            "not found"
        }
    );
    debug!(
        "   hvcc_from_magic: {}",
        if hvcc_from_magic.is_some() {
            "found"
        } else {
            "not found"
        }
    );

    let hvcc_data = hvcc_from_recursive.or(hvcc_from_magic);

    if let Some(hvcc_data) = hvcc_data {
        debug!("   hvcc_data.len: {}", hvcc_data.len());

        if hvcc_data.len() >= 20 {
            let Some(b) = hvcc_data.get(1) else {
                warn!("☢️ [CORRUPTION] HEIC hvcC box truncated: missing profile_idc");
                return Err(ImgQualityError::AnalysisError("hvcC truncated".to_string()));
            };
            let profile_idc = b & 0x1F;

            let mut compat_bytes = [0u8; 4];
            for (i, byte) in compat_bytes.iter_mut().enumerate() {
                let Some(b) = hvcc_data.get(2 + i) else {
                    warn!(
                        "☢️ [CORRUPTION] HEIC hvcC box truncated at compatibility flags byte {}",
                        i
                    );
                    return Err(ImgQualityError::AnalysisError(
                        "hvcC flags truncated".to_string(),
                    ));
                };
                *byte = *b;
            }
            let compat_flags = u32::from_be_bytes(compat_bytes);

            // HEVCDecoderConfigurationRecord fixed fields
            let Some(b_16) = hvcc_data.get(16) else {
                warn!("☢️ [CORRUPTION] HEIC hvcC box truncated at chroma_format_idc");
                return Err(ImgQualityError::AnalysisError(
                    "hvcC chroma truncated".to_string(),
                ));
            };
            let chroma_format_idc = b_16 & 0x03;

            let Some(byte_17) = hvcc_data.get(17) else {
                warn!("☢️ [CORRUPTION] HEIC hvcC box truncated at bit_depth field");
                return Err(ImgQualityError::AnalysisError(
                    "hvcC bit_depth truncated".to_string(),
                ));
            };
            let bit_depth_luma = ((byte_17 >> 5_i32) & 0x07) + 8;
            let bit_depth_chroma = (byte_17 & 0x07) + 8;

            // Dimension 0: chromaFormatIdc — direct chroma subsampling
            // 4:2:0 (1) or 4:2:2 (2) → definitively lossy (HEVC lossless requires 4:4:4)
            if chroma_format_idc == crate::constants::HEIC_CHROMA_420
                || chroma_format_idc == crate::constants::HEIC_CHROMA_422
            {
                return Ok(false);
            }

            // Dimension 1: Main/Main10/MainStillPicture → always 4:2:0 → always lossy
            if profile_idc == crate::constants::HEIC_PROFILE_MAIN
                || profile_idc == crate::constants::HEIC_PROFILE_MAIN10
                || profile_idc == crate::constants::HEIC_PROFILE_MAIN_STILL
            {
                return Ok(false);
            }

            // Dimension 2: RExt (4) or SCC (9) profiles can be lossless
            if profile_idc == crate::constants::HEIC_PROFILE_REXT
                || profile_idc == crate::constants::HEIC_PROFILE_SCC
            {
                let is_444 = chroma_format_idc == crate::constants::HEIC_CHROMA_444;

                // Check colr box for Identity matrix (RGB = lossless indicator for RExt)
                let has_rgb_identity_matrix = find_box_payload_by_magic(data, *b"colr")
                    .or_else(|| find_box_data_recursive(data, *b"colr"))
                    .and_then(|colr_data| {
                        if colr_data.len() >= 11 && colr_data.get(0..4) == Some(b"nclx") {
                            let b1 = *colr_data.get(8)?;
                            let b2 = *colr_data.get(9)?;
                            Some(u16::from_be_bytes([b1, b2]))
                        } else {
                            None
                        }
                    })
                    .is_some_and(|matrix| matrix == 0);

                if has_rgb_identity_matrix {
                    return Ok(true);
                }

                // Check pixi box for high bit depth
                let has_high_bitdepth = find_box_payload_by_magic(data, *b"pixi")
                    .or_else(|| find_box_data_recursive(data, *b"pixi"))
                    .and_then(|pixi_data| {
                        if pixi_data.is_empty() {
                            None
                        } else {
                            let Some(num_ch) = crate::numeric_cast::u8_to_usize_strict(
                                *pixi_data.first()?,
                                "heic_pixi_num_ch",
                            ) else {
                                crate::progress_mode::emit_stderr("☢️ [ANOMALY] HEIC pixi num_ch overflow! Refusing to forge data.");
                                return None;
                            };
                            if num_ch > 0 && pixi_data.len() > num_ch {
                                Some(pixi_data.get(1..=num_ch)?.iter().copied().max()?)
                            } else {
                                None
                            }
                        }
                    })
                    .is_some_and(|max_depth| max_depth >= crate::constants::HEIC_LOSSLESS_MIN_BIT_DEPTH);

                if has_high_bitdepth {
                    return Ok(true);
                }

                // High bit depth from hvcC itself
                if is_444
                    && (bit_depth_luma >= crate::constants::HEIC_LOSSLESS_MIN_BIT_DEPTH
                        || bit_depth_chroma >= crate::constants::HEIC_LOSSLESS_MIN_BIT_DEPTH)
                {
                    return Ok(true);
                }

                // RExt/SCC + 4:4:4 without other indicators — likely lossless
                if is_444 {
                    return Ok(true);
                }

                // RExt/SCC without 4:4:4 — ambiguous (RExt can also do lossy 4:2:0)
                return Err(ImgQualityError::AnalysisError(format!(
                    "HEIC: RExt/SCC profile ({}) without 4:4:4 chroma; cannot determine — {}",
                    profile_idc,
                    path.display()
                )));
            }

            // Dimension 4: Check profile compatibility flags — bit 4 = RExt compatible
            if (compat_flags & (1 << (31_i32 - i32::from(crate::constants::HEIC_PROFILE_REXT))))
                != 0
            {
                if chroma_format_idc == crate::constants::HEIC_CHROMA_444 {
                    return Ok(true);
                }
                return Err(ImgQualityError::AnalysisError(format!(
                    "HEIC: RExt compatibility flag set but chroma {} (not 4:4:4); cannot determine — {}",
                    chroma_format_idc,
                    path.display()
                )));
            }

            // Dimension 5: Parse SPS NAL units to check transquant_bypass_enabled_flag
            if let Some(is_lossless) = detect_heic_lossless_via_mp4parse_data(data)
                && is_lossless
            {
                return Ok(true);
            }

            // Unknown profile but hvcC exists — profiles 5-8, 10+ are rare
            // Most are lossy variants; treat as lossy rather than Err (safe default)
            if std::env::var("IMGQUALITY_VERBOSE").is_ok() {
                eprintln!("   📊 HEIC: unknown profile {profile_idc} — treating as lossy");
            }
            return Ok(false);
        }
    }

    // No hvcC box — fallback to lossy (safe default for HEIC)
    Ok(false)
}

fn detect_heic_lossless_via_mp4parse_data(data: &[u8]) -> Option<bool> {
    let hvcc_data = find_box_data_recursive(data, *b"hvcC")?;
    parse_sps_for_transquant_bypass_flag(hvcc_data)
}

fn parse_sps_for_transquant_bypass_flag(hvcc_data: &[u8]) -> Option<bool> {
    if hvcc_data.len() < 25 {
        return None;
    }
    let num_nalu_arrays = if let Some(b) = hvcc_data.get(24) {
        crate::numeric_cast::u8_to_usize_sat(*b)
    } else {
        warn!("☢️ [CORRUPTION] hvcC box too short to read num_nalu_arrays");
        return None;
    };
    let mut pos = 25;
    for _ in 0..num_nalu_arrays {
        if pos + 3 > hvcc_data.len() {
            return None;
        }
        let nal_unit_type = if let Some(b) = hvcc_data.get(pos) {
            b & 0x3F
        } else {
            warn!(
                "☢️ [CORRUPTION] hvcC box truncated at NAL unit type byte at {}",
                pos
            );
            return None;
        };
        let b1 = if let Some(b) = hvcc_data.get(pos + 1) {
            *b
        } else {
            warn!(
                "☢️ [CORRUPTION] hvcC box truncated at NAL unit length high byte at {}",
                pos + 1
            );
            return None;
        };
        let b2 = if let Some(b) = hvcc_data.get(pos + 2) {
            *b
        } else {
            warn!(
                "☢️ [CORRUPTION] hvcC box truncated at NAL unit length low byte at {}",
                pos + 2
            );
            return None;
        };
        let num_nalus = crate::numeric_cast::u16_to_usize_sat(u16::from_be_bytes([b1, b2]));
        pos += 3;
        if nal_unit_type == crate::constants::HEIC_NAL_UNIT_TYPE_SPS {
            for _ in 0..num_nalus {
                if pos + 2 > hvcc_data.len() {
                    return None;
                }
                let b1 = *hvcc_data.get(pos)?;
                let b2 = *hvcc_data.get(pos + 1)?;
                let Some(nal_unit_length) = crate::numeric_cast::u16_to_usize_strict(
                    u16::from_be_bytes([b1, b2]),
                    "heic_nal_len",
                ) else {
                    crate::progress_mode::emit_stderr(
                        "☢️ [ANOMALY] HEIC NAL unit length overflow! Refusing to forge data.",
                    );
                    return None;
                };
                pos += 2;
                if pos + nal_unit_length > hvcc_data.len() {
                    return None;
                }
                let sps_payload = hvcc_data.get(pos..pos + nal_unit_length)?;
                pos += nal_unit_length;
                if sps_payload.len() < 3 {
                    continue;
                }
                return parse_sps_rbsp_for_transquant_bypass(sps_payload);
            }
        } else {
            for _ in 0..num_nalus {
                if pos + 2 > hvcc_data.len() {
                    return None;
                }
                let b1 = *hvcc_data.get(pos)?;
                let b2 = *hvcc_data.get(pos + 1)?;
                let nal_unit_length =
                    crate::numeric_cast::u16_to_usize_sat(u16::from_be_bytes([b1, b2]));
                pos += 2 + nal_unit_length;
            }
        }
    }
    None
}

fn parse_sps_rbsp_for_transquant_bypass(sps_payload: &[u8]) -> Option<bool> {
    struct BitReader<'a> {
        data: &'a [u8],
        bit_pos: usize,
    }
    impl<'a> BitReader<'a> {
        const fn new(data: &'a [u8]) -> Self {
            BitReader { data, bit_pos: 0 }
        }
        fn read_bits(&mut self, n: usize) -> Option<u32> {
            if self.bit_pos + n > self.data.len() * 8 {
                return None;
            }
            let mut value = 0u32;
            for i in 0..n {
                let byte_pos = (self.bit_pos + i) / 8;
                let bit_offset = 7 - ((self.bit_pos + i) % 8);
                if byte_pos < self.data.len() {
                    let bit = (*self.data.get(byte_pos)? >> bit_offset) & 1;
                    value = (value << 1_i32) | u32::from(bit);
                }
            }
            self.bit_pos += n;
            Some(value)
        }
        fn read_ue(&mut self) -> Option<u32> {
            let mut leading_zeros = 0u32;
            while self.bit_pos < self.data.len() * 8 {
                let bit = self.read_bits(1)?;
                if bit == 1 {
                    break;
                }
                leading_zeros += 1;
            }
            let info = if leading_zeros > 0 {
                self.read_bits(crate::numeric_cast::u32_to_usize_sat(leading_zeros))?
            } else {
                0
            };
            Some((1 << leading_zeros) - 1 + info)
        }
    }

    if sps_payload.len() < 3 {
        return None;
    }
    let rbsp = sps_payload.get(2..)?;
    let mut reader = BitReader::new(rbsp);
    reader.read_bits(4)?; // sps_video_parameter_set_id
    let max_sub_layers = reader.read_bits(3)?;
    reader.read_bits(1)?; // sps_temporal_id_nesting_flag
    reader.read_ue()?; // sps_seq_parameter_set_id
    let chroma_format = reader.read_ue()?;
    if chroma_format == 3 {
        reader.read_bits(1)?;
    } // separate_colour_plane_flag
    reader.read_ue()?; // pic_width_in_luma_samples
    reader.read_ue()?; // pic_height_in_luma_samples
    if reader.read_bits(1)? == 1 {
        // conformance_window_flag
        for _ in 0_i32..4_i32 {
            reader.read_ue()?;
        }
    }
    reader.read_ue()?; // bit_depth_luma_minus8
    reader.read_ue()?; // bit_depth_chroma_minus8
    for _ in 0..=max_sub_layers {
        reader.read_ue()?;
    } // sps_max_dec_pic_buffering_minus1
    for _ in 0..=max_sub_layers {
        reader.read_ue()?;
    } // sps_max_num_reorder_pics
    for _ in 0..=max_sub_layers {
        reader.read_ue()?;
    } // sps_max_latency_increase_plus1
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
    if reader.read_bits(1)? == 1 {
        // pcm_enabled_flag
        reader.read_bits(1)?;
        reader.read_bits(1)?;
        reader.read_ue()?;
        reader.read_ue()?;
        reader.read_bits(1)?;
    }
    let transquant_bypass = reader.read_bits(1)?;
    Some(transquant_bypass == 1)
}

/// Multi-dimensional HEIC analysis (using both libheif and metadata inspection).
///
/// # Errors
/// Returns an error if the file is corrupted or analysis fails.
// Rationale: This function handles complex, sequential initialization or business logic where further fragmentation would hinder readability and maintainability.
#[allow(
    clippy::too_many_lines,
    reason = "Complex orchestration logic where fragmenting state into smaller helpers would decrease readability and increase cognitive overhead."
)]
pub fn analyze_heic_file_v4(path: &Path) -> Result<(DynamicImage, HeicAnalysis)> {
    let lib_heif = LibHeif::new();

    // 🛡️ Create security limits BEFORE reading the file
    #[cfg(feature = "v1_21")]
    let mut limits = libheif_rs::SecurityLimits::default();

    #[cfg(feature = "v1_21")]
    {
        // Set to 15GB memory limit for large/complex HEIC files (e.g., from Weibo)
        limits.set_max_total_memory(crate::constants::HEIC_MAX_MEMORY_LIMIT);

        // Increase ipco box child limit from default 100 to 50000
        // This fixes "Maximum number of child boxes (100) in 'ipco' box exceeded" errors
        limits.set_max_children_per_box(crate::constants::HEIC_MAX_CHILDREN_PER_BOX);

        // Increase other limits for complex HEIC files
        limits.set_max_items(crate::constants::HEIC_MAX_ITEMS);
        limits.set_max_components(crate::constants::HEIC_MAX_COMPONENTS);
        limits.set_max_iloc_extents_per_item(crate::constants::HEIC_MAX_EXTENTS);
    }

    let data = std::fs::read(path)?;

    // Create empty context first
    let mut ctx = HeifContext::new().map_err(|e| {
        ImgQualityError::ImageReadError(format!("Failed to create HEIC context: {e}"))
    })?;

    // Set security limits BEFORE reading data
    #[cfg(feature = "v1_21")]
    {
        ctx.set_security_limits(&limits).map_err(|e| {
            ImgQualityError::ImageReadError(format!("Failed to set security limits: {e}"))
        })?;
    }

    // Now read the data with security limits applied
    ctx.read_bytes(&data)
        .or_else(|e| {
            let error_msg = format!("{e}");
            // Fallback: Scan for 'ftyp' manually if NoFtypBox error
            if error_msg.contains("NoFtypBox") || error_msg.contains("No 'ftyp' box") {
                // Fallback 1: Try to find ftyp box manually
                if let Some(pos) = data.windows(4).position(|w| w == b"ftyp")
                    && pos >= 4
                {
                    let Some(sliced_data) = data.get(pos - 4..) else {
                        warn!(
                            "☢️ [ANOMALY] HEIC data truncated before 'ftyp' box at position {}",
                            pos
                        );
                        return Err(e);
                    };
                    if matches!(ctx.read_bytes(sliced_data), Ok(())) {
                        return Ok(());
                    }
                }

                // Fallback 2: Try file-based reading (doesn't require holding data reference)
                if let Some(path_str) = path.to_str() {
                    // Create a new context for file-based reading
                    if let Ok(mut file_ctx) = HeifContext::new() {
                        // Set security limits on the new context
                        #[cfg(feature = "v1_21")]
                        {
                            file_ctx.set_security_limits(&limits)?;
                        }

                        // Try to read from file path
                        if matches!(file_ctx.read_file(path_str), Ok(())) {
                            // Replace ctx with the successfully loaded file_ctx
                            ctx = file_ctx;
                            return Ok(());
                        }
                    }
                }
            }
            Err(e)
        })
        .map_err(|e| {
            let error_msg = format!("{e}");
            if error_msg.contains("SecurityLimitExceeded") || error_msg.contains("ipco") {
                ImgQualityError::ImageReadError(format!(
                    "HEIC security limit exceeded (ipco box limit): {e}"
                ))
            } else {
                ImgQualityError::ImageReadError(format!("[CRITICAL-HEIC-V4-FAIL] {e}"))
            }
        })?;

    let handle = ctx.primary_image_handle().map_err(|e| {
        ImgQualityError::ImageReadError(format!("Failed to get primary image: {e}"))
    })?;

    let width = handle.width();
    let height = handle.height();
    let has_alpha = handle.has_alpha_channel();
    let bit_depth = handle.luma_bits_per_pixel();

    let is_lossless_result = detect_heic_is_lossless(&data, path);
    if std::env::var("IMGQUALITY_VERBOSE").is_ok() {
        eprintln!("   📊 HEIC detect_heic_is_lossless result: {is_lossless_result:?}");
    }
    let is_lossless = is_lossless_result.unwrap_or_else(|_| {
        tracing::debug!("Missing HEIC lossless info; defaulting to false");
        false
    });

    // Detect HDR and Dolby Vision
    let mut is_hdr = false;
    let mut is_dolby_vision = false;

    // Quick scan for HDR/DV boxes in the already read data
    if let Some(colr_data) = find_box_data_recursive(&data, *b"colr")
        && colr_data.len() >= 11
        && colr_data.get(0..4) == Some(b"nclx")
    {
        let primaries = u16::from_be_bytes([colr_data[4], colr_data[5]]);
        let transfer = u16::from_be_bytes([colr_data[6], colr_data[7]]);
        if primaries == 9 && (transfer == 16 || transfer == 18) {
            is_hdr = true;
        }
    }
    if find_box_data_recursive(&data, *b"dvcC").is_some()
        || find_box_data_recursive(&data, *b"dvvC").is_some()
    {
        is_dolby_vision = true;
        is_hdr = true;
    }

    let image_count = ctx.image_ids().len();
    let has_auxiliary = handle.number_of_depth_images() > 0_i32;

    // Detect gainmap and vendor-specific metadata from XMP in raw HEIC data
    let xmp_str = extract_xmp_from_heic_data(&data);
    let has_gainmap = xmp_str.as_deref().is_some_and(|xmp: &str| {
        xmp.contains("hdrgm:")
            || xmp.contains("GainMap")
            || xmp.contains("gainmap")
            || (xmp.contains("GCamera:") && xmp.contains("HDR"))
    });
    let has_vendor_metadata = xmp_str.as_deref().is_some_and(|xmp: &str| {
        xmp.contains("urn:samsung:image:")
            || xmp.contains("GCamera:")
            || xmp.contains("com.google.android.camera")
    });

    let decoded_image = lib_heif
        .decode(&handle, ColorSpace::Rgb(RgbChroma::Rgb), None)
        .map_err(|e| ImgQualityError::ImageReadError(format!("Failed to decode HEIC: {e}")))?;

    let planes = decoded_image.planes();
    let plane = planes
        .interleaved
        .ok_or_else(|| ImgQualityError::ImageReadError("No RGB plane found".to_string()))?;

    let img = image::RgbImage::from_raw(width, height, plane.data.to_vec())
        .map(DynamicImage::ImageRgb8)
        .ok_or_else(|| ImgQualityError::ImageReadError("Failed to create RGB image".to_string()))?;

    let codec = "HEVC".to_string();

    let analysis = HeicAnalysis {
        bit_depth: Some(bit_depth),
        codec,
        is_lossless,
        has_alpha,
        image_count,
        hdr: HeicHdrInfo {
            is_hdr,
            is_dolby_vision,
            has_gainmap,
        },
        aux: HeicAuxInfo {
            has_auxiliary,
            has_vendor_metadata,
        },
    };

    debug!(
        file = %path.display(),
        width = width,
        height = height,
        bit_depth = bit_depth,
        is_lossless = is_lossless,
        is_hdr = is_hdr,
        is_dv = is_dolby_vision,
        "HEIC analysis complete"
    );

    Ok((img, analysis))
}

#[must_use]
pub fn is_heic_file(path: &Path) -> bool {
    // Rely strictly on magic bytes, NOT extensions, to avoid deep analysis failures (e.g. NoFtypBox)
    // on files that just happen to have a .heic extension but contain different format data.
    if let Ok(mut file) = std::fs::File::open(path) {
        use std::io::Read;
        let mut buffer = [0u8; 12];
        if file.read_exact(&mut buffer).is_ok() && &buffer[4..8] == b"ftyp" {
            let brand = &buffer[8..12];
            if matches!(
                brand,
                b"heic"
                    | b"heix"
                    | b"heim"
                    | b"heis"
                    | b"hevc"
                    | b"hevx"
                    | b"hev1"
                    | b"mif1"
                    | b"msf1"
            ) {
                return true;
            }
        }
    }
    false
}

/// Extract XMP packet string from raw HEIC/HEIF binary data.
///
/// HEIC files store XMP in an `xml ` item. This scanner looks for the canonical
/// XMP packet header (`<?xpacket begin`) or the `<x:xmpmeta` root element directly
/// in the raw bytes (they are always UTF-8 / ASCII-compatible).
///
/// Returns `None` if no XMP data is found.
/// # Panics
/// Panics if the HEIC data is corrupted and XMP markers are found but the following data is inaccessible.
#[must_use]
pub fn extract_xmp_from_heic_data(data: &[u8]) -> Option<String> {
    // 🛡️ Security: Limit total data size to 100MB for XMP scanning to prevent timeouts
    if crate::numeric_cast::usize_to_u64(data.len()) > crate::constants::HEIC_MAX_XMP_SCAN_BYTES {
        return None;
    }

    // XMP packet starts with <?xpacket begin or <x:xmpmeta or <rdf:RDF
    let markers: &[&[u8]] = &[b"<?xpacket begin", b"<x:xmpmeta", b"<rdf:RDF"];
    for marker in markers {
        if let Some(start) = data.windows(marker.len()).position(|w| w == *marker) {
            // XMP is always UTF-8; grab up to 64 KB from the start marker
            let end = (start + crate::constants::HEIC_XMP_GRAB_BYTES).min(data.len());
            return String::from_utf8_lossy(data.get(start..end)?)
                .into_owned()
                .into();
        }
    }
    None
}

/// Fallback: find box payload by direct magic byte search.
/// This handles cases where boxes are inside full boxes (e.g. meta box with version/flags)
/// that `find_box_data_recursive` may not handle correctly.
fn find_box_payload_by_magic(data: &[u8], box_type: [u8; 4]) -> Option<&[u8]> {
    if let Some(pos) = data.windows(4).position(|w| w == box_type)
        && pos >= 4
    {
        let mut size_bytes = [0u8; 4];
        for (i, byte) in size_bytes.iter_mut().enumerate() {
            *byte = if let Some(b) = data.get(pos - 4 + i) {
                *b
            } else {
                warn!("☢️ [CORRUPTION] Truncated box size before type at {}", pos);
                return None;
            };
        }
        let Some(size) = crate::numeric_cast::u32_to_usize_strict(
            u32::from_be_bytes(size_bytes),
            "heic_box_size",
        ) else {
            crate::progress_mode::emit_stderr(
                "☢️ [ANOMALY] HEIC box size overflow! Refusing to forge data.",
            );
            return None;
        };
        if size >= 8 && pos + size - 4 <= data.len() {
            return data.get(pos + 4..pos - 4 + size);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::Builder;

    #[test]
    fn test_is_heic_file() {
        let mut heic_asset_builder = Builder::new()
            .suffix(".heic")
            .tempfile()
            .unwrap_or_else(|e| panic!("create temp heic: {e:?}"));
        heic_asset_builder
            .write_all(&[0, 0, 0, 12, b'f', b't', b'y', b'p', b'h', b'e', b'i', b'c'])
            .unwrap_or_else(|e| panic!("write heic header: {e:?}"));

        let mut heif_sample_builder = Builder::new()
            .suffix(".HEIF")
            .tempfile()
            .unwrap_or_else(|e| panic!("create temp heif: {e:?}"));
        heif_sample_builder
            .write_all(&[0, 0, 0, 12, b'f', b't', b'y', b'p', b'm', b'i', b'f', b'1'])
            .unwrap_or_else(|e| panic!("write heif header: {e:?}"));

        let mut jpg = Builder::new()
            .suffix(".jpg")
            .tempfile()
            .unwrap_or_else(|e| panic!("create temp jpg: {e:?}"));
        jpg.write_all(&[0, 0, 0, 12, b'f', b't', b'y', b'p', b'j', b'p', b'e', b'g'])
            .unwrap_or_else(|e| panic!("write jpg header: {e:?}"));

        assert!(is_heic_file(heic_asset_builder.path()));
        assert!(is_heic_file(heif_sample_builder.path()));
        assert!(!is_heic_file(jpg.path()));
        assert!(!is_heic_file(Path::new("test.heic")));
    }
}
