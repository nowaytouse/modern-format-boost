//! HEIC/HEIF Format Analysis Module
//!
//! Uses libheif-rs to decode and analyze HEIC/HEIF images

use crate::common_utils::{find_all_box_data_recursive, find_box_data_recursive};
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

/// Classify HEIC/HEIF compression from positive codec evidence.
///
/// Evidence ladder (positive proof only):
/// 1. **hvcC `chroma_format_idc` 4:2:0 / 4:2:2** → `Lossy` (subsampling
///    discards chroma information).
/// 2. Multiple hvcC records → `Lossy` only when every record independently
///    proves 4:2:0/4:2:2; mixed primary/auxiliary evidence stays `Unknown`.
/// 3. **RExt/SCC + monochrome/4:4:4 + PPS
///    `transquant_bypass_enabled_flag == 0`** →
///    `Lossy` (quantization bypass disabled: every CU is quantized).
/// 4. **RExt/SCC + monochrome/4:4:4 + PPS bypass == 1** → `Unknown`: the flag only
///    permits per-CU bypass; it does not prove every coded unit used it.
/// 5. **RExt/SCC + monochrome/4:4:4 but PPS unparsable** → `Unknown` (insufficient
///    evidence; previously fabricated as an error or lossy).
/// 6. **`profile_idc` outside 1-4/9** (reserved profiles) → `Unknown`
///    (previously silently defaulted to lossy).
///
/// Missing/truncated hvcC → `Err` (malformed still HEIC).
///
/// # Errors
/// Returns an error if the hvcC box is missing or truncated, or heif-info
/// authoritative validation rejects a structurally suspicious file.
// Rationale: This function handles complex, sequential initialization or
// business logic where further fragmentation would hinder readability and
// maintainability.
pub fn classify_heic_compression(
    data: &[u8],
    path: &Path,
) -> Result<crate::image_detection::CompressionType> {
    use crate::image_detection::CompressionType;

    let hvcc_boxes = find_all_box_data_recursive(data, *b"hvcC");

    crate::log_debug!(
        crate::infra::static_logs::messages::LABEL_HEIC_AUDIT,
        &format!(
            "Checking lossless status for '{}' | Forensic: hvcc_records={}",
            path.display(),
            hvcc_boxes.len()
        )
    );

    if hvcc_boxes.len() > 1 {
        // HEIF may carry independent primary, thumbnail and auxiliary codec
        // configurations. Only admit the container when every hvcC record has
        // direct chroma-subsampling proof; otherwise the primary item's
        // compression semantics are not established without resolving ipma.
        for hvcc_data in &hvcc_boxes {
            if hvcc_data.len() < 20 {
                return Err(ImgQualityError::AnalysisError(format!(
                    "HEIC: hvcC box is {} bytes (minimum 20 required); cannot determine compression — {}",
                    hvcc_data.len(),
                    path.display()
                )));
            }
            extract_hevc_bit_depths(hvcc_data)?;
            let chroma_format_idc = hvcc_data[16] & 0x03;
            if chroma_format_idc != crate::constants::HEIC_CHROMA_420
                && chroma_format_idc != crate::constants::HEIC_CHROMA_422
            {
                return Ok(CompressionType::Unknown);
            }
        }
        return Ok(CompressionType::Lossy);
    }

    let hvcc_data = hvcc_boxes.first().copied();

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
                            "hvcC box truncated at compatibility flags byte {i} (Forensic \
                             Recovery Failed)"
                        )
                    );
                    return Err(ImgQualityError::AnalysisError(
                        "hvcC flags truncated".to_string(),
                    ));
                };
                *byte = *b;
            }
            let _compat_flags = u32::from_be_bytes(compat_bytes);

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
            extract_hevc_bit_depths(hvcc_data)?;

            // Dimension 0: chromaFormatIdc — direct chroma subsampling
            // 4:2:0 (1) or 4:2:2 (2) → definitively lossy (HEVC lossless requires 4:4:4)
            if chroma_format_idc == crate::constants::HEIC_CHROMA_420
                || chroma_format_idc == crate::constants::HEIC_CHROMA_422
            {
                return Ok(CompressionType::Lossy);
            }

            // Main/Main10/MainStillPicture with a contradictory non-4:2:0
            // hvcC record is malformed/ambiguous, not positive lossy evidence.
            if profile_idc == crate::constants::HEIC_PROFILE_MAIN
                || profile_idc == crate::constants::HEIC_PROFILE_MAIN10
                || profile_idc == crate::constants::HEIC_PROFILE_MAIN_STILL
            {
                return Ok(CompressionType::Unknown);
            }

            // Dimension 1: RExt (4) or SCC (9) profiles can be lossless
            if profile_idc == crate::constants::HEIC_PROFILE_REXT
                || profile_idc == crate::constants::HEIC_PROFILE_SCC
            {
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

                // 4:2:0/4:2:2 already returned above. Both monochrome and
                // 4:4:4 can use HEVC lossless coding, so neither is lossy
                // evidence by itself; inspect PPS instead.

                // LAYER 2: PPS transquant_bypass_enabled_flag Check
                // If it is 1, it only PERMITS per-CU lossless coding. A PPS flag
                // cannot prove that every CU/slice used bypass, regardless of
                // sign-data-hiding or encoder profile.
                let pps_flags = check_heic_pps_transquant_bypass_flag(data);
                if let Some((transquant_bypass_enabled, sign_data_hiding_enabled)) = pps_flags {
                    if transquant_bypass_enabled {
                        crate::log_debug!(
                            crate::infra::static_logs::messages::LABEL_HEIC_AUDIT,
                            &format!(
                                "HEVC PPS analysis | transquant_bypass_enabled_flag=1, \
                                 sign_data_hiding_enabled_flag={} for '{}'; per-CU usage is unknown",
                                u8::from(sign_data_hiding_enabled),
                                path.display()
                            )
                        );
                        // Attempt heif-info validation if available
                        if let Some(validation_result) = try_heif_info_validation(path) {
                            crate::log_debug!(
                                crate::infra::static_logs::messages::LABEL_HEIC_AUDIT,
                                &format!(
                                    "heif-info validation result for '{}': {}",
                                    path.display(),
                                    if validation_result { "PASS" } else { "FAIL" }
                                )
                            );
                            if !validation_result {
                                crate::media_conversion_gate::probe_image_format_batch_audit(
                                    "probe_heic",
                                    format!(
                                        "heif-info validation failed for '{}' despite favorable PPS flags; \
                                         failing closed as malformed",
                                        path.display()
                                    ),
                                );
                                return Err(ImgQualityError::AnalysisError(format!(
                                    "HEIC: heif-info rejected file with lossless PPS evidence — {}",
                                    path.display()
                                )));
                            }
                        }
                        return Ok(CompressionType::Unknown);
                    }
                    // Flag is 0 -> definitely lossy
                    return Ok(CompressionType::Lossy);
                }

                let matrix_warning = if has_rgb_identity_matrix {
                    " (Note: Identity RGB matrix detected, but PPS parsing failed)"
                } else {
                    ""
                };
                crate::media_conversion_gate::probe_image_format_batch_audit(
                    "probe_heic",
                    format!(
                        "Ambiguous RExt/SCC profile detected | Forensic: profile_idc={} is \
                                 4:4:4 but PPS transquant_bypass_enabled_flag could not be parsed for '{}'{}; \
                                 compression evidence is inconclusive",
                        profile_idc,
                        path.display(),
                        matrix_warning
                    ),
                );
                // 4:4:4 RExt/SCC with unparsable PPS: insufficient evidence —
                // Unknown, never a fabricated lossy/lossless verdict.
                return Ok(CompressionType::Unknown);
            }

            // Unknown profile but hvcC exists — profiles 5-8, 10+ are reserved
            // by the HEVC spec: no codec-evidence ladder applies, so the
            // compression semantics are unproven. Unknown, never a fabricated
            // "safe default lossy".
            crate::media_conversion_gate::probe_image_format_batch_audit(
                "probe_heic",
                format!(
                    "Reserved HEVC profile_idc={} for '{}': compression evidence unavailable",
                    profile_idc,
                    path.display()
                ),
            );
            return Ok(CompressionType::Unknown);
        }

        // hvcC present but shorter than the fixed HEVCDecoderConfigurationRecord
        // fields we parse: truncated container, not a "lossy" verdict.
        return Err(ImgQualityError::AnalysisError(format!(
            "HEIC: hvcC box is {} bytes (minimum 20 required); cannot determine compression — {}",
            hvcc_data.len(),
            path.display()
        )));
    }

    // No hvcC box: a HEIC without an HEVC configuration record is malformed
    // (AVIF's equivalent probe errs the same way). Refuse to fabricate "lossy".
    Err(ImgQualityError::AnalysisError(format!(
        "HEIC: no hvcC configuration box found; cannot determine compression — {}",
        path.display()
    )))
}

/// Try to validate HEIC file using heif-info tool (authoritative libheif-based validator).
/// Returns Some(true) if validation passes, Some(false) if fails, None if tool unavailable.
fn try_heif_info_validation(path: &Path) -> Option<bool> {
    let tool_path = crate::common_utils::resolve_tool_path(crate::constants::TOOL_HEIF_INFO)?;

    let output = match std::process::Command::new(&tool_path)
        .arg(crate::safe_path_arg(path).as_ref())
        .output()
    {
        Ok(out) => out,
        _ => return None,
    };

    if output.status.success() {
        // heif-info succeeded - file is structurally valid
        Some(true)
    } else {
        // heif-info failed - file may be corrupted or not truly HEIF compliant
        crate::log_debug!(
            crate::infra::static_logs::messages::LABEL_HEIC_AUDIT,
            &format!(
                "heif-info validation failed for '{}': exit_code={:?}, stderr={}",
                path.display(),
                output.status.code(),
                String::from_utf8_lossy(&output.stderr).trim()
            )
        );
        Some(false)
    }
}

fn check_heic_pps_transquant_bypass_flag(data: &[u8]) -> Option<(bool, bool)> {
    let hvcc_data = find_box_data_recursive(data, *b"hvcC")?;
    parse_pps_for_transquant_bypass_flag(hvcc_data)
}

fn parse_pps_for_transquant_bypass_flag(hvcc_data: &[u8]) -> Option<(bool, bool)> {
    if hvcc_data.len() < 23 {
        return None;
    }
    let num_nalu_arrays = if let Some(b) = hvcc_data.get(22) {
        crate::numeric_cast::u8_to_usize_strict(*b, "num_nalu_arrays")?
    } else {
        crate::log_corruption!(
            crate::infra::static_logs::messages::LABEL_HEIC_AUDIT,
            "hvcC box too short to read num_nalu_arrays"
        );
        return None;
    };
    let mut pos = 23;
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
                    "hvcC box truncated at NAL unit length high byte at position {} (Forensic \
                     Scan Aborted)",
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
        if nal_unit_type == crate::constants::HEIC_NAL_UNIT_TYPE_PPS {
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
                let pps_payload = hvcc_data.get(pos..pos + nal_unit_length)?;
                pos += nal_unit_length;
                if pps_payload.len() < 3 {
                    continue;
                }
                return parse_pps_rbsp_for_transquant_bypass(pps_payload);
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

fn parse_pps_rbsp_for_transquant_bypass(pps_payload: &[u8]) -> Option<(bool, bool)> {
    if pps_payload.len() < 3 {
        return None;
    }
    // Skip NAL unit header (2 bytes) to reach RBSP payload
    let raw_rbsp = pps_payload.get(2..)?;

    // Remove Emulation Prevention Bytes (0x03)
    let mut rbsp = Vec::with_capacity(raw_rbsp.len());
    let mut i = 0;
    while i < raw_rbsp.len() {
        if i + 2 < raw_rbsp.len()
            && raw_rbsp[i] == 0x00
            && raw_rbsp[i + 1] == 0x00
            && raw_rbsp[i + 2] == 0x03
        {
            rbsp.push(0x00);
            rbsp.push(0x00);
            i += 3;
        } else {
            rbsp.push(raw_rbsp[i]);
            i += 1;
        }
    }

    let mut reader = BitReader::new(&rbsp);

    // H.265 §7.3.2.3 pic_parameter_set_rbsp() — fields in spec order:
    reader.read_ue()?; // pps_pic_parameter_set_id  ue(v)
    reader.read_ue()?; // pps_seq_parameter_set_id  ue(v)
    reader.read_bits(1)?; // dependent_slice_segments_enabled_flag  u(1)
    reader.read_bits(1)?; // output_flag_present_flag  u(1)
    reader.read_bits(3)?; // num_extra_slice_header_bits  u(3)
    let sign_data_hiding_enabled_flag = reader.read_bits(1)?; // u(1) — must be 0 for lossless
    reader.read_bits(1)?; // cabac_init_present_flag  u(1)
    reader.read_ue()?; // num_ref_idx_l0_default_active_minus1  ue(v)
    reader.read_ue()?; // num_ref_idx_l1_default_active_minus1  ue(v)
    // SE fields: SE uses identical Exp-Golomb bit encoding as UE (H.265 §9.1);
    // only the value interpretation differs — safe to read_ue() to advance the bit pointer.
    reader.read_ue()?; // init_qp_minus26  se(v) — skip
    reader.read_bits(1)?; // constrained_intra_pred_flag  u(1)
    reader.read_bits(1)?; // transform_skip_enabled_flag  u(1)
    let cu_qp_delta_enabled_flag = reader.read_bits(1)? == 1;
    if cu_qp_delta_enabled_flag {
        reader.read_ue()?; // diff_cu_qp_delta_depth  ue(v)
    }
    reader.read_ue()?; // pps_cb_qp_offset  se(v) — skip
    reader.read_ue()?; // pps_cr_qp_offset  se(v) — skip
    reader.read_bits(1)?; // pps_slice_chroma_qp_offsets_present_flag  u(1)
    reader.read_bits(1)?; // weighted_pred_flag  u(1)
    reader.read_bits(1)?; // weighted_bipred_flag  u(1)
    let transquant_bypass_enabled_flag = reader.read_bits(1)?; // u(1)

    Some((
        transquant_bypass_enabled_flag == 1,
        sign_data_hiding_enabled_flag == 1,
    ))
}

#[cfg(feature = "v1_21")]
fn project_heif_security_limits() -> libheif_rs::SecurityLimits {
    let mut limits = libheif_rs::SecurityLimits::default();
    limits.set_max_total_memory(crate::constants::HEIC_MAX_MEMORY_LIMIT);
    limits.set_max_children_per_box(crate::constants::HEIC_MAX_CHILDREN_PER_BOX);
    limits.set_max_items(crate::constants::HEIC_MAX_ITEMS);
    limits.set_max_components(crate::constants::HEIC_MAX_COMPONENTS);
    limits.set_max_iloc_extents_per_item(crate::constants::HEIC_MAX_EXTENTS);
    limits
}

/// Parse HEIF bytes with the same explicit limits used by the full HEIC decoder.
///
/// # Errors
/// Returns an error when the context cannot be created, configured, or parsed.
pub(crate) fn read_heif_context_with_project_limits(data: &[u8]) -> Result<HeifContext<'_>> {
    let mut ctx = HeifContext::new().map_err(|error| {
        ImgQualityError::ImageReadError(format!("Failed to create HEIF context: {error}"))
    })?;

    #[cfg(feature = "v1_21")]
    ctx.set_security_limits(&project_heif_security_limits())
        .map_err(|error| {
            ImgQualityError::ImageReadError(format!(
                "Failed to configure HEIF project security limits: {error}"
            ))
        })?;

    ctx.read_bytes(data).map_err(|error| {
        ImgQualityError::ImageReadError(format!(
            "Failed to parse HEIF with project security limits: {error}"
        ))
    })?;
    Ok(ctx)
}

/// Multi-dimensional HEIC analysis (using both libheif and metadata
/// inspection).
///
/// # Errors
/// Returns an error if the file is corrupted or analysis fails.
// Rationale: This function handles complex, sequential initialization or
// business logic where further fragmentation would hinder readability and
// maintainability.
pub fn analyze_heic_file_v4(path: &Path) -> Result<(DynamicImage, HeicAnalysis)> {
    let lib_heif = LibHeif::new();

    #[cfg(feature = "v1_21")]
    let limits = project_heif_security_limits();

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
                format!(
                    "HEIC AUDIT: Failed to set security limits | Forensic: {e} (Constraint \
                     Violation)"
                ),
            );
            ImgQualityError::ImageReadError(format!("Failed to set security limits: {e}"))
        })?;
    }

    // Now read the data with security limits applied
    let read_res = match ctx.read_bytes(&data) {
        Ok(()) => Ok(()),
        Err(e) => {
            // `final_err` is only reassigned under the v1_21 feature (security-limit
            // retry).
            #[cfg_attr(not(feature = "v1_21"), expect(unused_mut))]
            let mut final_err = e;
            let error_msg = format!("{final_err}");
            let mut recovered = false;
            if error_msg.contains("NoFtypBox") || error_msg.contains("No 'ftyp' box") {
                // File-based reading may use libheif's native I/O path, but never
                // reinterpret an embedded `ftyp` byte sequence as a new file root.
                if let Some(path_str) = path.to_str() {
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
                                    "failed to create fallback HEIF context during ftyp recovery: \
                                     {ctx_err}"
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

    let compression_result = classify_heic_compression(&data, path);
    if std::env::var(crate::constants::ENV_VERBOSE).is_ok() {
        crate::log_debug!(
            crate::infra::static_logs::messages::LABEL_HEIC_AUDIT,
            &format!("classify_heic_compression result: {compression_result:?}")
        );
    }
    let compression = compression_result?;
    let is_lossless = compression == crate::image_detection::CompressionType::Lossless;

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
            "HEIC AUDIT: Missing RGB plane | Forensic: planes.interleaved is None after decoding \
             (Format Incompatibility)",
        );
        ImgQualityError::ImageReadError("No RGB plane found".to_string())
    })?;

    let img = image::RgbImage::from_raw(width, height, plane.data.to_vec())
        .map(DynamicImage::ImageRgb8)
        .ok_or_else(|| {
            crate::media_conversion_gate::probe_image_format_batch_audit(
                "probe_heic",
                format!(
                    "HEIC AUDIT: Pixel data integrity failure | Forensic: RgbImage::from_raw \
                     failed for {width}x{height} buffer (Corruption Detected)"
                ),
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
            "Analysis complete for '{}' | Forensic: {}x{}, bit_depth={}, lossless={}, hdr={}, \
             dv={}",
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
    // Rely strictly on magic bytes, NOT extensions, to avoid deep analysis failures
    // (e.g. NoFtypBox) on files that just happen to have a .heic extension but
    // contain different format data.
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
/// XMP packet header (`<?xpacket begin`) or the `<x:xmpmeta` root element
/// directly in the raw bytes (they are always UTF-8 / ASCII-compatible).
///
/// Returns `None` if no XMP data is found.
/// # Panics
/// Panics if the HEIC data is corrupted and XMP markers are found but the
/// following data is inaccessible.
#[must_use]
pub fn extract_xmp_from_heic_data(data: &[u8]) -> Option<String> {
    // 🛡️ Security: Limit total data size to 100MB for XMP scanning to prevent
    // timeouts
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

/// Find a box payload through the shared boundary-checked ISOBMFF walker.
#[must_use]
pub fn find_box_payload_by_magic(data: &[u8], box_type: [u8; 4]) -> Option<&[u8]> {
    crate::common_utils::find_box_data_recursive(data, box_type)
}

#[cfg(test)]
mod tests {
    include!("../../tests/internal/heic_analysis.rs");
}
