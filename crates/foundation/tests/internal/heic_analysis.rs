// SMOKE TESTS: Basic sanity checks for HEIC analysis.
// These tests verify basic control flow and magic byte detection but do not
// perform deep forensic validation on real media assets.
use super::*;

#[test]
fn smoke_is_heic_file() {
    use std::io::Write;
    use tempfile::Builder;

    let mut heic_test_builder = Builder::new()
        .suffix(".heic")
        .tempfile()
        .unwrap_or_else(|_| panic!("create temp heic"));
    heic_test_builder
        .write_all(&[0, 0, 0, 12, b'f', b't', b'y', b'p', b'h', b'e', b'i', b'c'])
        .unwrap_or_else(|_| panic!("write heic header"));

    let mut heif_sample_builder = Builder::new()
        .suffix(".HEIF")
        .tempfile()
        .unwrap_or_else(|_| panic!("create temp heif"));
    heif_sample_builder
        .write_all(&[0, 0, 0, 12, b'f', b't', b'y', b'p', b'm', b'i', b'f', b'1'])
        .unwrap_or_else(|_| panic!("write heif header"));

    assert!(is_heic_file(heic_test_builder.path()).expect("HEIC magic probe"));
    assert!(is_heic_file(heif_sample_builder.path()).expect("HEIF magic probe"));

    let mut jpeg_builder = Builder::new().suffix(".jpg").tempfile().unwrap();
    jpeg_builder.write_all(&[0xFF, 0xD8, 0xFF]).unwrap();
    assert!(!is_heic_file(jpeg_builder.path()).expect("JPEG magic probe"));
}

#[test]
fn is_heic_file_missing_file_returns_error_not_false() {
    let dir = tempfile::tempdir().expect("tempdir");
    let missing = dir.path().join("missing.heic");

    let err = is_heic_file(&missing).expect_err("missing HEIC probe target must be an error");

    assert!(err.to_string().contains("missing.heic"));
}

#[cfg(feature = "v1_21")]
mod hvc_tests {
    use super::*;

    #[test]
    fn smoke_heic_bit_depth_parsing_honest() {
        let mut hvcc = vec![0u8; 20];
        // Byte 17: bit_depth_luma_minus8 (bits 0-2)
        // Byte 18: bit_depth_chroma_minus8 (bits 0-2)
        hvcc[17] = 0b0000_0010; // 10-bit luma (2 + 8)
        hvcc[18] = 0b0000_0100; // 12-bit chroma (4 + 8)

        let (luma, chroma) = extract_hevc_bit_depths(&hvcc).expect("Production function failed");
        assert_eq!(
            luma, 10,
            "Failed to extract correct luma depth from Byte 17"
        );
        assert_eq!(
            chroma, 12,
            "Failed to extract correct chroma depth from Byte 18"
        );

        // Verify it doesn't cross-contaminate
        hvcc[17] = 0b1110_0010; // High bits set, low bits = 2
        let (luma_only, _) = extract_hevc_bit_depths(&hvcc).unwrap();
        assert_eq!(
            luma_only, 10,
            "Luma depth extraction was polluted by high bits"
        );
    }

    #[test]
    fn smoke_hevc_sps_parser_sync_simple_no_profile() {
        // VPS=0, Layers=1 (max_sub_layers_minus1), Nesting=1
        let mut rbsp = vec![0u8; 32];
        rbsp[0] = 0b0000_0011;

        // Header ends at bit 8 (Byte 1).
        // General PTL + Level: 96 bits. Ends at bit 104 (Byte 13).
        // Bits 104 to 120 (Byte 15): Reserved + sub-layer loop.

        // Byte 13: [0][0][000000] -> sub_layer_profile_present=0, sub_layer_level_present=0
        rbsp[13] = 0b0000_0000;

        // Target ue(0) starts at bit 120 (Byte 15)
        // 0b1000_0000
        rbsp[15] = 0b1000_0000;

        let mut reader = BitReader::new(&rbsp);
        reader.read_bits(4).unwrap();
        let layers = reader.read_bits(3).unwrap();
        let _ = reader.read_bits(1).unwrap();

        reader
            .skip_profile_tier_level(true, layers)
            .expect("Failed to skip simple PTL");

        let pos_after = reader.bit_pos;
        let sps_id = reader.read_ue();
        assert_eq!(
            sps_id,
            Some(0),
            "Bitstream desync at bit {pos_after} (expected 120)"
        );
    }

    #[test]
    fn smoke_hevc_sps_parser_sync_complex() {
        // VPS=0, Layers=1 (max_sub_layers_minus1), Nesting=1
        let mut rbsp = vec![0u8; 64];
        rbsp[0] = 0b0000_0011;

        // Byte 13: [1][0][000000] -> sub_layer_profile_present=1, sub_layer_level_present=0
        rbsp[13] = 0b1000_0000;

        // Bits 106 to 120: Reserved 2bits for i=1 to 7 (14 bits).

        // Sub-layer details start at bit 120.
        // sub_layer_profile_present[0]=1 -> skip 88 bits.
        // Ends at bit 120 + 88 = 208 (Byte 26).

        // Byte 26: Target ue(0) -> 0b1000_0000
        rbsp[26] = 0b1000_0000;

        let mut reader = BitReader::new(&rbsp);
        reader.read_bits(4).unwrap();
        let layers = reader.read_bits(3).unwrap();
        let _ = reader.read_bits(1).unwrap();

        reader
            .skip_profile_tier_level(true, layers)
            .expect("Failed to skip complex PTL");

        let pos_after = reader.bit_pos;
        let sps_id = reader.read_ue();
        assert_eq!(
            sps_id,
            Some(0),
            "Bitstream desync at bit {pos_after} (expected 208)"
        );
    }
}

#[test]
fn smoke_extract_xmp_from_heic_data() {
    let mut data = vec![0u8; 100];
    let xmp_content = "<x:xmpmeta>test</x:xmpmeta>";
    let start_pos = 20;
    data[start_pos..start_pos + xmp_content.len()].copy_from_slice(xmp_content.as_bytes());

    let extracted = extract_xmp_from_heic_data(&data).unwrap();
    assert!(extracted.starts_with(xmp_content));
}

#[test]
fn smoke_find_box_payload_by_magic() {
    let magic = *b"test";
    let payload = b"hello";
    let mut data = 13u32.to_be_bytes().to_vec();
    data.extend_from_slice(&magic);
    data.extend_from_slice(payload);

    let result = find_box_payload_by_magic(&data, magic).unwrap();
    assert_eq!(result, payload);

    // Test truncated box
    let truncated_data = &data[..10];
    assert!(find_box_payload_by_magic(truncated_data, magic).is_none());
}

#[test]
fn smoke_detect_heic_is_lossless_simple_lossy() {
    // 4:2:0 YUV is always lossy in HEVC
    let mut data = vec![0u8; 100];
    let hvcc_magic = *b"hvcC";
    let pos = 20;

    let mut hvcc_payload = vec![0u8; 20];
    hvcc_payload[1] = 1; // profile_idc = Main (lossy)
    hvcc_payload[16] = 1; // chroma_format_idc = 4:2:0 (lossy)

    let size = (u32::try_from(hvcc_payload.len()).unwrap() + 8).to_be_bytes();
    data[pos - 4..pos].copy_from_slice(&size);
    data[pos..pos + 4].copy_from_slice(&hvcc_magic);
    data[pos + 4..pos + 4 + hvcc_payload.len()].copy_from_slice(&hvcc_payload);

    let path = std::path::Path::new("test.heic");
    assert!(!detect_heic_is_lossless(&data, path).unwrap());
}

#[test]
fn test_control_group_lossless_lossy() {
    let lossless_path = std::path::Path::new("/tmp/test_lossless.heic");
    let lossy_path = std::path::Path::new("/tmp/test_lossy.heic");
    if lossless_path.exists() && lossy_path.exists() {
        let lossless_data = std::fs::read(lossless_path).unwrap();
        let lossy_data = std::fs::read(lossy_path).unwrap();
        
        let lossless_res = detect_heic_is_lossless(&lossless_data, lossless_path);
        let lossy_res = detect_heic_is_lossless(&lossy_data, lossy_path);
        
        println!("Control group: lossless={lossless_res:?}, lossy={lossy_res:?}");
        assert!(lossless_res.unwrap(), "Lossless HEIC failed detection");
        assert!(!lossy_res.unwrap(), "Lossy HEIC failed detection");
    }
}
