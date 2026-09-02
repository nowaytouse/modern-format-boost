use foundation::image_formats::webp::is_lossless_from_bytes;
use foundation::image_heic_analysis::{
    BitReader, extract_hevc_bit_depths, extract_xmp_from_heic_data,
};
use foundation::image_jpeg_analysis::is_jpeg_complete;

#[test]
fn ci_and_installer_pin_libheif_required_by_the_rust_binding() {
    for source in [
        include_str!("../../../../.github/workflows/ci-quality.yml"),
        include_str!("../../src/bin/install_media_dependencies.rs"),
    ] {
        assert!(source.contains("libheif-1.23.1"));
        assert!(!source.contains("libheif-1.21.0"));
    }
}

#[test]
fn product_release_workflows_publish_only_macos_arm64() {
    for source in [
        include_str!("../../../../.github/workflows/cd-nightly.yml"),
        include_str!("../../../../.github/workflows/cd-stable.yml"),
    ] {
        assert!(source.contains("aarch64-apple-darwin"));
        assert!(source.contains("libheif"));
        assert!(source.contains("libmpc"));
        for forbidden in [
            "x86_64-apple-darwin",
            "x86_64-unknown-linux-gnu",
            "x86_64-pc-windows-msvc",
        ] {
            assert!(
                !source.contains(forbidden),
                "product release workflow still contains {forbidden}"
            );
        }
    }
}

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

    // Verify it doesn't cross-contaminate (the old bug)
    hvcc[17] = 0b1110_0010; // High bits set, low bits = 2
    let (luma_only, _) = extract_hevc_bit_depths(&hvcc).unwrap();
    assert_eq!(
        luma_only, 10,
        "Luma depth extraction was polluted by high bits of the same byte"
    );
}

#[test]
fn smoke_hevc_sps_parser_sync_complex() {
    // Construction ensuring MSB-first alignment:
    // Byte 0: [0000][001][1] -> VPS=0, Layers=1 (max_sub_layers_minus1), Nesting=1
    let mut rbsp = vec![0u8; 64];
    rbsp[0] = 0b0000_0011;

    // Header ends at bit 8 (Byte 1).
    // General PTL + Level: 96 bits. Ends at bit 104 (Byte 13).

    // Byte 13: [1][0][000000] -> sub_layer_profile_present=1, sub_layer_level_present=0
    // Flags are bits 104, 105.
    rbsp[13] = 0b1000_0000;

    // Bits 106 to 120: Reserved 2bits for i=1 to 7 (14 bits).
    // Loop finishes at bit 120 (Byte 15).

    // Sub-layer details start at bit 120.
    // sub_layer_profile_present[0]=1 -> skip 88 bits.
    // sub_layer_level_present[0]=0 -> skip 0 bits.
    // Total sub-layer skip = 88 bits. Ends at bit 120 + 88 = 208 (Byte 26).

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

#[test]
fn smoke_extract_xmp_from_heic_data_precise() {
    let mut data = vec![0u8; 100];
    let xmp_content = "<x:xmpmeta>test</x:xmpmeta>";
    let start_marker = "http://ns.adobe.com/xap/1.0/\0";
    let start_pos = 20;

    // Mock HEIC XMP structure: marker + content
    let full_content = format!("{start_marker}{xmp_content}");
    data[start_pos..start_pos + full_content.len()].copy_from_slice(full_content.as_bytes());

    let extracted = extract_xmp_from_heic_data(&data).unwrap();
    // We expect the extraction to start from the XML tag, not the marker
    assert!(
        extracted.starts_with(xmp_content),
        "Extraction did not start with XML tag"
    );
}

#[test]
fn smoke_webp_structural_defense_collision() {
    // Lossy WebP with "VP8L" hidden in pixel data. RIFF size must be
    // structurally exact (24 = "WEBP" + 8-byte chunk header + 12-byte payload)
    // so the probe classifies via the real VP8 chunk, not a fail-open default.
    let mut data = Vec::new();
    data.extend_from_slice(b"RIFF");
    data.extend_from_slice(&(24u32.to_le_bytes())); // Size
    data.extend_from_slice(b"WEBP");

    // Real lossy chunk
    data.extend_from_slice(b"VP8 ");
    data.extend_from_slice(&(12u32.to_le_bytes())); // Chunk size
    data.extend_from_slice(&[0x56, 0x50, 0x38, 0x4C, 0, 0, 0, 0, 0, 0, 0, 0]); // Contains "VP8L" bytes

    assert!(
        !is_lossless_from_bytes(&data).expect("lossy WebP with decoy bytes must parse"),
        "WebP parser was fooled by fake VP8L bytes in pixel data"
    );
}

#[test]
fn smoke_jpeg_integrity_with_thumbnail() {
    let mut data = Vec::new();
    data.extend_from_slice(&[0xFF, 0xD8]); // Main SOI

    // App1 with thumbnail
    data.extend_from_slice(&[0xFF, 0xE1]);
    data.extend_from_slice(&[0x00, 0x10]); // Length
    data.extend_from_slice(&[0xFF, 0xD8]); // Thumbnail SOI
    data.extend_from_slice(&[0x00, 0x00]); // Data
    data.extend_from_slice(&[0xFF, 0xD9]); // Thumbnail EOI

    // Main image truncated here (missing SOS and EOI)
    assert!(
        !is_jpeg_complete(&data),
        "JPEG checker fooled by thumbnail EOI (missing SOS)"
    );

    // Add main SOS and EOI
    data.extend_from_slice(&[0xFF, 0xDA]); // Main SOS
    data.extend_from_slice(&[0xFF, 0xD9]); // Main EOI
    assert!(
        is_jpeg_complete(&data),
        "JPEG checker failed to recognize complete image with thumbnail"
    );
}
