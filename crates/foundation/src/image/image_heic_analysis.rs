//! HEIC/HEIF Format Analysis Module
//!
//! Uses libheif-rs to decode and analyze HEIC/HEIF images

use crate::common_utils::find_box_data_recursive;
use crate::unified_error::{ImgQualityError, Result};
use image::DynamicImage;
use libheif_rs::{ColorSpace, HeifContext, LibHeif, RgbChroma};
use serde::{Deserialize, Serialize};
use std::path::Path;

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

/// Extract bit depths from hvcC configuration record.
///
/// # Errors
/// Returns an error if the hvcC configuration record is truncated.
pub fn extract_hevc_bit_depths(hvcc_data: &[u8]) -> Result<(u8, u8)> {
    let Some(byte_17) = hvcc_data.get(17) else {
        return Err(ImgQualityError::AnalysisError(
            "hvcC bit_depth_luma truncated".to_string(),
        ));
    };
    let bit_depth_luma = (byte_17 & 0x07) + 8;

    let Some(byte_18) = hvcc_data.get(18) else {
        return Err(ImgQualityError::AnalysisError(
            "hvcC bit_depth_chroma truncated".to_string(),
        ));
    };
    let bit_depth_chroma = (byte_18 & 0x07) + 8;

    Ok((bit_depth_luma, bit_depth_chroma))
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
pub fn detect_heic_is_lossless(data: &[u8], path: &Path) -> Result<bool> {
    // Try find_box_data_recursive first, then fallback to direct magic byte search
    // This handles cases where boxes are inside full boxes (e.g. meta box with version/flags)
    let hvcc_from_recursive = find_box_data_recursive(data, *b"hvcC");
    let hvcc_from_magic = find_box_payload_by_magic(data, *b"hvcC");

    crate::log_debug!(
        crate::infra::static_logs::messages::LABEL_HEIC_AUDIT,
        &format!(
            "Checking lossless status for '{}' | Forensic: hvcc_recursive={}, hvcc_magic={}",
            path.display(),
            hvcc_from_recursive.is_some(),
            hvcc_from_magic.is_some()
        )
    );

    let hvcc_data = hvcc_from_recursive.or(hvcc_from_magic);

    if let Some(hvcc_data) = hvcc_data {
        crate::log_debug!(
            crate::infra::static_logs::messages::LABEL_HEIC_AUDIT,
            &format!(
                "hvcC payload isolated | Size: {} bytes (Forensic Analysis Initiated)",
                hvcc_data.len()
            )
        );

        if hvcc_data.len() >= 20 {
            let Some(b) = hvcc_data.get(1) else {
                crate::log_corruption!(
                    crate::infra::static_logs::messages::LABEL_HEIC_AUDIT,
                    "hvcC box truncated: missing profile_idc (Forensic Recovery Failed)"
                );
                return Err(ImgQualityError::AnalysisError("hvcC truncated".to_string()));
            };
            let profile_idc = b & 0x1F;

            let mut compat_bytes = [0u8; 4];
            for (i, byte) in compat_bytes.iter_mut().enumerate() {
                let Some(b) = hvcc_data.get(2 + i) else {
                    crate::log_corruption!(
                        crate::infra::static_logs::messages::LABEL_HEIC_AUDIT,
                        &format!(
                            "hvcC box truncated at compatibility flags byte {i} (Forensic Recovery Failed)"
                        )
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
                crate::log_corruption!(
                    crate::infra::static_logs::messages::LABEL_HEIC_AUDIT,
                    "hvcC box truncated at chroma_format_idc (Forensic Recovery Failed)"
                );
                return Err(ImgQualityError::AnalysisError(
                    "hvcC chroma truncated".to_string(),
                ));
            };
            let chroma_format_idc = b_16 & 0x03;

            let (bit_depth_luma, bit_depth_chroma) = extract_hevc_bit_depths(hvcc_data)?;

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
                let colr_payload = match find_box_payload_by_magic(data, *b"colr") {
                    Some(v) => Some(v),
                    None => find_box_data_recursive(data, *b"colr"),
                };
                let has_rgb_identity_matrix = colr_payload
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
                let pixi_payload = match find_box_payload_by_magic(data, *b"pixi") {
                    Some(v) => Some(v),
                    None => find_box_data_recursive(data, *b"pixi"),
                };
                let has_high_bitdepth = pixi_payload
                    .and_then(|pixi_data| {
                        // pixi is a FullBox: version(1) + flags(3) + num_channels(1) + bits_per_channel(num_channels)
                        if pixi_data.len() < 5 {
                            None
                        } else {
                            let Some(num_ch) = crate::numeric_cast::u8_to_usize_strict(
                                *pixi_data.get(4)?,
                                "heic_pixi_num_ch",
                            ) else {
                                crate::media_conversion_gate::probe_image_format_batch_audit(
                                    "probe_heic",
                                    "HEIC pixi num_ch overflow! Refusing to forge data.",
                                );
                                return None;
                            };
                            if num_ch > 0 && pixi_data.len() >= 5 + num_ch {
                                Some(pixi_data.get(5..5 + num_ch)?.iter().copied().max()?)
                            } else {
                                None
                            }
                        }
                    })
                    .is_some_and(|max_depth| {
                        max_depth >= crate::constants::HEIC_LOSSLESS_MIN_BIT_DEPTH
                    });

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
                crate::media_conversion_gate::probe_image_format_batch_audit(
                    "probe_heic",
                    format!(
                        "Ambiguous RExt/SCC profile detected | Forensic: profile_idc={} without 4:4:4 chroma for '{}'; precision detection is inconclusive",
                        profile_idc,
                        path.display()
                    ),
                );
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
                crate::media_conversion_gate::probe_image_format_audit(
                    "probe_heic",
                    path,
                    format!(
                        "HEIC AUDIT: RExt compatibility flag mismatch | Forensic: flag set but chroma {} is not 4:4:4 for '{}'; refusing to assume structural losslessness",
                        chroma_format_idc,
                        path.display()
                    ),
                );
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
            if std::env::var(crate::constants::ENV_VERBOSE).is_ok() {
                crate::log_debug!(
                    crate::infra::static_logs::messages::LABEL_HEIC_AUDIT,
                    &format!(
                        "Forensic: Unknown profile IDC {profile_idc} detected; defaulting to lossy conversion logic for structural safety"
                    )
                );
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
        crate::numeric_cast::u8_to_usize_strict(*b, "num_nalu_arrays")?
    } else {
        crate::log_corruption!(
            crate::infra::static_logs::messages::LABEL_HEIC_AUDIT,
            "hvcC box too short to read num_nalu_arrays"
        );
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
            crate::log_corruption!(
                crate::infra::static_logs::messages::LABEL_HEIC_AUDIT,
                &format!("hvcC box truncated at NAL unit type byte at {pos}")
            );
            return None;
        };
        let b1 = if let Some(b) = hvcc_data.get(pos + 1) {
            *b
        } else {
            crate::log_corruption!(
                crate::infra::static_logs::messages::LABEL_HEIC_AUDIT,
                &format!(
                    "hvcC box truncated at NAL unit length high byte at position {} (Forensic Scan Aborted)",
                    pos + 1
                )
            );
            return None;
        };
        let b2 = if let Some(b) = hvcc_data.get(pos + 2) {
            *b
        } else {
            crate::log_corruption!(
                crate::infra::static_logs::messages::LABEL_HEIC_AUDIT,
                &format!(
                    "hvcC box truncated at NAL unit length low byte at {}",
                    pos + 2
                )
            );
            return None;
        };
        let num_nalus = crate::numeric_cast::u16_to_usize_strict(
            u16::from_be_bytes([b1, b2]),
            "heic_nalu_count",
        )?;
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
                    "nal_unit_length",
                ) else {
                    crate::media_conversion_gate::probe_image_format_batch_audit(
                        "probe_heic",
                        "HEIC NAL unit length overflow! Refusing to forge data.",
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
                let Some(nal_unit_length) = crate::numeric_cast::u16_to_usize_strict(
                    u16::from_be_bytes([b1, b2]),
                    "nal_unit_length",
                ) else {
                    crate::media_conversion_gate::probe_image_format_batch_audit(
                        "probe_heic",
                        "HEIC NAL unit length overflow! Refusing to forge data.",
                    );
                    return None;
                };
                pos += 2 + nal_unit_length;
            }
        }
    }
    None
}

pub struct BitReader<'a> {
    pub data: &'a [u8],
    pub bit_pos: usize,
}
impl<'a> BitReader<'a> {
    #[must_use]
    pub const fn new(data: &'a [u8]) -> Self {
        BitReader { data, bit_pos: 0 }
    }
    pub fn read_bits(&mut self, n: usize) -> Option<u32> {
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
    pub const fn skip_bits(&mut self, n: usize) -> Option<()> {
        if self.bit_pos + n > self.data.len() * 8 {
            return None;
        }
        self.bit_pos += n;
        Some(())
    }

    pub fn read_ue(&mut self) -> Option<u32> {
        let mut leading_zeros = 0u32;
        while self.bit_pos < self.data.len() * 8 {
            let bit = self.read_bits(1)?;
            if bit == 1 {
                break;
            }
            leading_zeros += 1;
        }
        let info = if leading_zeros > 0 {
            self.read_bits(crate::numeric_cast::u32_to_usize_strict(
                leading_zeros,
                "leading_zeros",
            )?)?
        } else {
            0
        };
        Some((1u32.checked_shl(leading_zeros)?).saturating_sub(1) + info)
    }
    pub fn skip_profile_tier_level(
        &mut self,
        profile_present_flag: bool,
        max_num_sub_layers_minus1: u32,
    ) -> Option<()> {
        if profile_present_flag {
            self.skip_bits(2)?; // general_profile_space
            self.skip_bits(1)?; // general_tier_flag
            self.skip_bits(5)?; // general_profile_idc
            self.skip_bits(32)?; // general_profile_compatibility_flag
            self.skip_bits(48)?; // general_constraint_indicator_flags
        }
        self.skip_bits(8)?; // general_level_idc

        let mut sub_layer_profile_present = [false; 8];
        let mut sub_layer_level_present = [false; 8];

        let max_sub_layers = crate::numeric_cast::u32_to_usize_strict(
            max_num_sub_layers_minus1,
            "max_num_sub_layers_minus1",
        )?;

        for i in 0..max_sub_layers {
            if i < 8 {
                sub_layer_profile_present[i] = self.read_bits(1)? == 1;
                sub_layer_level_present[i] = self.read_bits(1)? == 1;
            }
        }

        if max_num_sub_layers_minus1 > 0 {
            for _ in max_num_sub_layers_minus1..8 {
                self.skip_bits(2)?; // reserved_zero_2bits
            }
        }

        for i in 0..max_sub_layers {
            if i < 8 {
                if sub_layer_profile_present[i] {
                    self.skip_bits(2)?; // sub_layer_profile_space
                    self.skip_bits(1)?; // sub_layer_tier_flag
                    self.skip_bits(5)?; // sub_layer_profile_idc
                    self.skip_bits(32)?; // sub_layer_profile_compatibility_flag
                    self.skip_bits(48)?; // sub_layer_constraint_indicator_flags
                }
                if sub_layer_level_present[i] {
                    self.skip_bits(8)?; // sub_layer_level_idc
                }
            }
        }
        Some(())
    }
}

fn parse_sps_rbsp_for_transquant_bypass(sps_payload: &[u8]) -> Option<bool> {
    if sps_payload.len() < 3 {
        return None;
    }
    let rbsp = sps_payload.get(2..)?;
    let mut reader = BitReader::new(rbsp);
    reader.read_bits(4)?; // sps_video_parameter_set_id
    let max_sub_layers = reader.read_bits(3)?;
    reader.read_bits(1)?; // sps_temporal_id_nesting_flag
    reader.skip_profile_tier_level(true, max_sub_layers)?;
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
    reader.read_ue()?; // log2_max_pic_order_cnt_lsb_minus4

    let sub_layer_ordering_info_present = reader.read_bits(1)? == 1;
    let start_idx = if sub_layer_ordering_info_present {
        0
    } else {
        max_sub_layers
    };

    for _ in start_idx..=max_sub_layers {
        reader.read_ue()?; // sps_max_dec_pic_buffering_minus1
        reader.read_ue()?; // sps_max_num_reorder_pics
        reader.read_ue()?; // sps_max_latency_increase_plus1
    }

    reader.read_ue()?; // sps_log2_min_luma_coding_block_size_minus3
    reader.read_ue()?; // sps_log2_diff_max_min_luma_coding_block_size
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
        crate::media_conversion_gate::probe_image_format_batch_audit(
            "probe_heic",
            format!("Context initialization failed | Forensic: {e}"),
        );
        ImgQualityError::ImageReadError(format!("Failed to create HEIC context: {e}"))
    })?;

    // Set security limits BEFORE reading data
    #[cfg(feature = "v1_21")]
    {
        ctx.set_security_limits(&limits).map_err(|e| {
            crate::media_conversion_gate::probe_image_format_batch_audit(
                "probe_heic",
                format!("HEIC AUDIT: Failed to set security limits | Forensic: {e} (Constraint Violation)"),
            );
            ImgQualityError::ImageReadError(format!("Failed to set security limits: {e}"))
        })?;
    }

    // Now read the data with security limits applied
    let read_res = match ctx.read_bytes(&data) {
        Ok(()) => Ok(()),
        Err(e) => {
            // `final_err` is only reassigned under the v1_21 feature (security-limit retry).
            #[cfg_attr(not(feature = "v1_21"), expect(unused_mut))]
            let mut final_err = e;
            let error_msg = format!("{final_err}");
            let mut recovered = false;
            let mut hard_fail = false;
            // Fallback: Scan for 'ftyp' manually if NoFtypBox error
            if error_msg.contains("NoFtypBox") || error_msg.contains("No 'ftyp' box") {
                // Fallback 1: Try to find ftyp box manually
                if let Some(pos) = data.windows(4).position(|w| w == b"ftyp")
                    && pos >= 4
                {
                    let sliced_data = data.get(pos - 4..);
                    crate::media_conversion_gate::probe_image_format_audit(
                        "probe_heic",
                        path,
                        format!(
                            "Data truncated before 'ftyp' box at position {} for '{}' | Forensic: Unexpected EOF during recovery scan; refusing to forge context to prevent downstream crash",
                            pos,
                            path.display()
                        ),
                    );
                    if let Some(sliced_data) = sliced_data {
                        if matches!(ctx.read_bytes(sliced_data), Ok(())) {
                            recovered = true;
                        }
                    } else {
                        hard_fail = true;
                    }
                }

                // Fallback 2: Try file-based reading (doesn't require holding data reference)
                if !recovered
                    && !hard_fail
                    && let Some(path_str) = path.to_str()
                {
                    // Create a new context for file-based reading
                    match HeifContext::new() {
                        Ok(mut file_ctx) => {
                            // Set security limits on the new context.
                            // Only reassigned under the v1_21 feature below.
                            #[cfg_attr(not(feature = "v1_21"), expect(unused_mut))]
                            let mut security_limits_ok = true;
                            #[cfg(feature = "v1_21")]
                            {
                                if let Err(set_err) = file_ctx.set_security_limits(&limits) {
                                    final_err = set_err;
                                    security_limits_ok = false;
                                }
                            }

                            // Try to read from file path
                            if security_limits_ok && matches!(file_ctx.read_file(path_str), Ok(()))
                            {
                                // Replace ctx with the successfully loaded file_ctx
                                ctx = file_ctx;
                                recovered = true;
                            }
                        }
                        Err(ctx_err) => {
                            crate::media_conversion_gate::probe_image_format_audit(
                                "probe_heic_context_new_failed",
                                path,
                                format!(
                                    "failed to create fallback HEIF context during ftyp recovery: {ctx_err}"
                                ),
                            );
                        }
                    }
                }
            }

            if recovered { Ok(()) } else { Err(final_err) }
        }
    };

    read_res.map_err(|e| {
        let error_msg = format!("{e}");
        crate::media_conversion_gate::probe_image_format_audit(
            "probe_heic",
            path,
            format!(
                "High-level read failed for '{}' | Forensic: {error_msg}",
                path.display()
            ),
        );
        if error_msg.contains("SecurityLimitExceeded") || error_msg.contains("ipco") {
            ImgQualityError::ImageReadError(format!(
                "HEIC security limit exceeded (ipco box limit): {e}"
            ))
        } else {
            ImgQualityError::ImageReadError(format!("[CRITICAL-HEIC-V4-FAIL] {e}"))
        }
    })?;

    let handle = ctx.primary_image_handle().map_err(|e| {
        crate::media_conversion_gate::probe_image_format_batch_audit(
            "probe_heic",
            format!("Failed to get primary image handle | Forensic: {e}"),
        );
        ImgQualityError::ImageReadError(format!("Failed to get primary image: {e}"))
    })?;

    let width = handle.width();
    let height = handle.height();
    let has_alpha = handle.has_alpha_channel();
    let bit_depth = handle.luma_bits_per_pixel();

    let is_lossless_result = detect_heic_is_lossless(&data, path);
    if std::env::var(crate::constants::ENV_VERBOSE).is_ok() {
        crate::log_debug!(
            crate::infra::static_logs::messages::LABEL_HEIC_AUDIT,
            &format!("detect_heic_is_lossless result: {is_lossless_result:?}")
        );
    }
    let is_lossless = is_lossless_result?;

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
        .map_err(|e| {
            crate::media_conversion_gate::probe_image_format_batch_audit(
                "probe_heic",
                format!("Decoding failed | Forensic: {e}"),
            );
            ImgQualityError::ImageReadError(format!("Failed to decode HEIC: {e}"))
        })?;

    let planes = decoded_image.planes();
    let plane = planes.interleaved.ok_or_else(|| {
        crate::media_conversion_gate::probe_image_format_batch_audit(
                "probe_heic",
                "HEIC AUDIT: Missing RGB plane | Forensic: planes.interleaved is None after decoding (Format Incompatibility)",
            );
        ImgQualityError::ImageReadError("No RGB plane found".to_string())
    })?;

    let img = image::RgbImage::from_raw(width, height, plane.data.to_vec())
        .map(DynamicImage::ImageRgb8)
        .ok_or_else(|| {
            crate::media_conversion_gate::probe_image_format_batch_audit(
                "probe_heic",
                format!("HEIC AUDIT: Pixel data integrity failure | Forensic: RgbImage::from_raw failed for {width}x{height} buffer (Corruption Detected)"),
            );
            ImgQualityError::ImageReadError("Failed to create RGB image".to_string())
        })?;

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

    crate::log_debug!(
        crate::infra::static_logs::messages::LABEL_HEIC_AUDIT,
        &format!(
            "Analysis complete for '{}' | Forensic: {}x{}, bit_depth={}, lossless={}, hdr={}, dv={}",
            path.display(),
            width,
            height,
            bit_depth,
            is_lossless,
            is_hdr,
            is_dolby_vision
        )
    );

    Ok((img, analysis))
}

pub fn is_heic_file(path: &Path) -> std::io::Result<bool> {
    // Rely strictly on magic bytes, NOT extensions, to avoid deep analysis failures (e.g. NoFtypBox)
    // on files that just happen to have a .heic extension but contain different format data.
    use std::io::Read;
    let mut file = std::fs::File::open(path).map_err(|err| {
        std::io::Error::new(
            err.kind(),
            format!(
                "failed to open HEIC magic probe target {}: {err}",
                path.display()
            ),
        )
    })?;
    let mut buffer = [0u8; 12];
    let n = file.read(&mut buffer).map_err(|err| {
        std::io::Error::new(
            err.kind(),
            format!(
                "failed to read HEIC magic probe target {}: {err}",
                path.display()
            ),
        )
    })?;
    if n >= 12 && &buffer[4..8] == b"ftyp" {
        let brand = &buffer[8..12];
        if matches!(
            brand,
            b"heic" | b"heix" | b"heim" | b"heis" | b"hevc" | b"hevx" | b"hev1" | b"mif1" | b"msf1"
        ) {
            return Ok(true);
        }
    }
    Ok(false)
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
#[must_use]
pub fn find_box_payload_by_magic(data: &[u8], box_type: [u8; 4]) -> Option<&[u8]> {
    if let Some(pos) = data.windows(4).position(|w| w == box_type)
        && pos >= 4
    {
        let mut size_bytes = [0u8; 4];
        for (i, byte) in size_bytes.iter_mut().enumerate() {
            *byte = if let Some(b) = data.get(pos - 4 + i) {
                *b
            } else {
                crate::log_corruption!(
                    crate::infra::static_logs::messages::LABEL_HEIC_AUDIT,
                    &format!(
                        "HEIC CORRUPTION AUDIT: Truncated box size before type at position {pos} | Forensic: Mandatory length field missing; refusing to forge data to prevent out-of-bounds scan"
                    )
                );
                return None;
            };
        }
        let Some(size) = crate::numeric_cast::u32_to_usize_strict(
            u32::from_be_bytes(size_bytes),
            "heic_box_size",
        ) else {
            crate::media_conversion_gate::probe_image_format_batch_audit(
                "probe_heic",
                "HEIC box size overflow! Refusing to forge data.",
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
    include!("../tests/heic_analysis.rs");
}
