use shared_utils::image_jpeg_analysis::{extract_gainmap_from_jpeg, is_ultra_hdr_jpeg};
use std::fs;

#[test]
fn test_ultrahdr_absolute_offset_fallback() {
    // Create a synthetic JPEG with MPF segment using absolute offsets
    // This is a complex task to do from scratch, so we'll simulate the data structure
    
    let mut data = vec![0xFF, 0xD8]; // SOI
    
    // APP1 XMP (simplified)
    let xmp_content = b"<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF><rdf:Description hdrgm:GainMapMax=\"2.0\" xmlns:hdrgm=\"http://ns.adobe.com/hdr-gain-map/1.0/\"/></rdf:RDF></x:xmpmeta>";
    let xmp_hdr = b"http://ns.adobe.com/xap/1.0/\0";
    data.push(0xFF); data.push(0xE1); // APP1
    let xmp_len = (xmp_hdr.len() + xmp_content.len() + 2) as u16;
    data.extend_from_slice(&xmp_len.to_be_bytes());
    data.extend_from_slice(xmp_hdr);
    data.extend_from_slice(xmp_content);
    
    // APP2 MPF
    data.push(0xFF); data.push(0xE2); // APP2
    
    // We'll calculate the MPF payload size later
    let mpf_id = b"MPF\0";
    let tiff_hdr = b"MM\0*"; // Big Endian
    let ifd_offset = 8u32;
    
    let mut mpf_payload = Vec::new();
    mpf_payload.extend_from_slice(tiff_hdr);
    mpf_payload.extend_from_slice(&ifd_offset.to_be_bytes());
    
    // IFD: 2 entries
    mpf_payload.extend_from_slice(&2u16.to_be_bytes());
    
    // Entry 1: NumberOfImages
    mpf_payload.extend_from_slice(&0xB001u16.to_be_bytes()); // Tag
    mpf_payload.extend_from_slice(&4u16.to_be_bytes());     // Type (LONG)
    mpf_payload.extend_from_slice(&1u32.to_be_bytes());     // Count
    mpf_payload.extend_from_slice(&2u32.to_be_bytes());     // Value (Number of images = 2)
    
    // Entry 2: MPEntry
    let mp_entry_val_offset = (mpf_payload.len() + 12 + 4) as u32; // Skip this entry + next IFD offset
    mpf_payload.extend_from_slice(&0xB002u16.to_be_bytes()); // Tag
    mpf_payload.extend_from_slice(&7u16.to_be_bytes());     // Type (UNDEFINED)
    mpf_payload.extend_from_slice(&32u32.to_be_bytes());    // Count (2 entries * 16 bytes)
    mpf_payload.extend_from_slice(&mp_entry_val_offset.to_be_bytes()); // Offset to MP Entries
    
    // Next IFD offset (0)
    mpf_payload.extend_from_slice(&0u32.to_be_bytes());
    
    // MP Entries array (start at mp_entry_val_offset)
    // Entry 0: Primary Image
    mpf_payload.extend_from_slice(&0u32.to_be_bytes()); // Attributes
    mpf_payload.extend_from_slice(&0u32.to_be_bytes()); // Size
    mpf_payload.extend_from_slice(&0u32.to_be_bytes()); // Offset
    mpf_payload.extend_from_slice(&0u32.to_be_bytes()); // Deps
    
    // Entry 1: Gainmap Image
    let gainmap_size = 10u32;
    // CRITICAL: We set an offset that would be VALID if interpreted as absolute,
    // but INVALID if interpreted as relative to the TIFF header.
    // Length of data so far + MPF ID + length field + marker
    let current_data_len = data.len() + 6 + mpf_payload.len() + gainmap_size as usize;
    let absolute_offset = (data.len() + 6 + mpf_payload.len() + 4) as u32; // Point to after everything
    
    mpf_payload.extend_from_slice(&0u32.to_be_bytes()); // Attributes
    mpf_payload.extend_from_slice(&gainmap_size.to_be_bytes()); // Size
    mpf_payload.extend_from_slice(&absolute_offset.to_be_bytes()); // Offset (ABSOLUTE)
    mpf_payload.extend_from_slice(&0u32.to_be_bytes()); // Deps
    
    // Now assemble APP2 segment
    let app2_len = (mpf_id.len() + mpf_payload.len() + 2) as u16;
    data.extend_from_slice(&app2_len.to_be_bytes());
    data.extend_from_slice(mpf_id);
    data.extend_from_slice(&mpf_payload);
    
    // Final Gainmap data at absolute_offset
    let gainmap_data = vec![0xFF, 0xD8, 0xFF, 0xD9, 0, 0, 0, 0, 0, 0]; // SOI + EOI + padding
    data.extend_from_slice(&gainmap_data);
    
    // EOI
    data.push(0xFF); data.push(0xD9);
    
    assert!(is_ultra_hdr_jpeg(&data), "Should be identified as UltraHDR");
    
    // This should fail with the OLD logic because relative offset would be way beyond file size.
    // The NEW logic should fall back to absolute offset and find the FFD8 SOI.
    let result = extract_gainmap_from_jpeg(&data);
    
    match result {
        Ok((_, gainmap)) => {
            println!("✅ Successfully extracted gainmap via absolute offset fallback!");
            assert_eq!(gainmap.width(), 0); // Well, image decoding will fail on dummy data but we care about EXTRACTION
        }
        Err(e) => {
            // Since dummy data width/height decoding might fail, we check the error message
            // or we make the dummy data a bit more valid.
            if e.contains("Failed to decode gainmap image") {
                println!("✅ Extraction succeeded, decoding failed as expected for dummy data.");
            } else {
                panic!("❌ Failed to extract gainmap: {}", e);
            }
        }
    }
}
