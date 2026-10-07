// Synthetic animated WebP without VP8X in header (classification / duration parser regression).

// Build a synthetic animated WebP without VP8X in its header.
//
// # Panics
// Panics if any payload length fails to fit in a u32.
#[must_use]
fn build_synthetic_animated_webp_without_vp8x_in_header() -> Vec<u8> {
    // Two ANMF chunks with a minimal 16-byte frame header.
    // Duration is a 24-bit little-endian integer at offset 12..15 in the ANMF payload.
    fn anmf_chunk(duration_ms: u32) -> Vec<u8> {
        let mut payload = vec![0u8; 16];
        if let Some(b) = payload.get_mut(12) {
            *b = foundation::numeric_cast::u32_shifted_byte_to_u8(duration_ms, 0);
        }
        if let Some(b) = payload.get_mut(13) {
            *b = foundation::numeric_cast::u32_shifted_byte_to_u8(duration_ms, 8);
        }
        if let Some(b) = payload.get_mut(14) {
            *b = foundation::numeric_cast::u32_shifted_byte_to_u8(duration_ms, 16);
        }
        payload.extend_from_slice(b"VP8L\0\0\0\0");
        let mut out = Vec::new();
        out.extend_from_slice(b"ANMF");
        out.extend_from_slice(
            &u32::try_from(payload.len())
                .expect("synthetic ANMF payload length fits u32")
                .to_le_bytes(),
        );
        out.extend_from_slice(&payload);
        // RIFF chunks are padded to even size (payload len 16 already even)
        out
    }

    // Minimal RIFF WEBP container with ANIM + ANMF markers placed beyond the first 64 bytes.
    // This does not need to be a decodable image; it exists to lock animation classification logic.
    let mut bytes = Vec::new();

    // RIFF header (size filled after all chunks are assembled).
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(b"WEBP");

    // Add a JUNK chunk to push animation metadata beyond the first 64 bytes
    // while keeping valid RIFF chunk structure (id + size + payload).
    let junk_payload = vec![0u8; 80];
    bytes.extend_from_slice(b"JUNK");
    bytes.extend_from_slice(
        &u32::try_from(junk_payload.len())
            .expect("synthetic JUNK payload length fits u32")
            .to_le_bytes(),
    );
    bytes.extend_from_slice(&junk_payload);

    // Insert ANIM chunk (with a small payload)
    let anim_payload = vec![0u8; 16];
    bytes.extend_from_slice(b"ANIM");
    bytes.extend_from_slice(
        &u32::try_from(anim_payload.len())
            .expect("synthetic ANIM payload length fits u32")
            .to_le_bytes(),
    );
    bytes.extend_from_slice(&anim_payload);

    bytes.extend_from_slice(&anmf_chunk(100));
    bytes.extend_from_slice(&anmf_chunk(120));

    let riff_size = u32::try_from(bytes.len() - 8).expect("synthetic RIFF size fits u32");
    bytes[4..8].copy_from_slice(&riff_size.to_le_bytes());

    bytes
}
