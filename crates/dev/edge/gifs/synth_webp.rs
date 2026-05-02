pub fn build_synthetic_animated_webp_without_vp8x_in_header() -> Vec<u8> {
    // Two ANMF chunks with a minimal 16-byte frame header.
    // Duration is a 24-bit little-endian integer at offset 12..15 in the ANMF payload.
    fn anmf_chunk(duration_ms: u32) -> Vec<u8> {
        let mut payload = vec![0u8; 16];
        if let Some(b) = payload.get_mut(12) { *b = (duration_ms & 0xFF) as u8; }
        if let Some(b) = payload.get_mut(13) { *b = ((duration_ms >> 8) & 0xFF) as u8; }
        if let Some(b) = payload.get_mut(14) { *b = ((duration_ms >> 16) & 0xFF) as u8; }
        let mut out = Vec::new();
        out.extend_from_slice(b"ANMF");
        out.extend_from_slice(&u32::try_from(payload.len()).unwrap_or(0).to_le_bytes());
        out.extend_from_slice(&payload);
        // RIFF chunks are padded to even size (payload len 16 already even)
        out
    }

    // Minimal RIFF WEBP container with ANIM + ANMF markers placed beyond the first 64 bytes.
    // This does not need to be a decodable image; it exists to lock animation classification logic.
    let mut bytes = Vec::new();

    // RIFF header (size placeholder; not used by our detectors)
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(b"WEBP");

    // Add a JUNK chunk to push animation metadata beyond the first 64 bytes
    // while keeping valid RIFF chunk structure (id + size + payload).
    let junk_payload = vec![0u8; 80];
    bytes.extend_from_slice(b"JUNK");
    bytes.extend_from_slice(&u32::try_from(junk_payload.len()).unwrap_or(0).to_le_bytes());
    bytes.extend_from_slice(&junk_payload);

    // Insert ANIM chunk (with a small payload)
    let anim_payload = vec![0u8; 16];
    bytes.extend_from_slice(b"ANIM");
    bytes.extend_from_slice(&u32::try_from(anim_payload.len()).unwrap_or(0).to_le_bytes());
    bytes.extend_from_slice(&anim_payload);

    bytes.extend_from_slice(&anmf_chunk(100));
    bytes.extend_from_slice(&anmf_chunk(120));

    bytes
}

