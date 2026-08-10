// Synthetic animated WebP for CI runtime probe regression (no external fixture).

/// Two-frame animated WebP (100×80) for header preflight / `detect_video` regression.
#[must_use]
fn build_synthetic_two_frame_animated_webp() -> Vec<u8> {
    fn anmf_chunk(duration_ms: u32) -> Vec<u8> {
        let mut payload = vec![0u8; 24];
        payload[12] = foundation::numeric_cast::u32_shifted_byte_to_u8(duration_ms, 0);
        payload[13] = foundation::numeric_cast::u32_shifted_byte_to_u8(duration_ms, 8);
        payload[14] = foundation::numeric_cast::u32_shifted_byte_to_u8(duration_ms, 16);
        payload.extend_from_slice(b"VP8L\x00\x00\x00\x00");
        let size = u32::try_from(payload.len()).expect("anmf payload fits u32");
        let mut chunk = b"ANMF".to_vec();
        chunk.extend_from_slice(&size.to_le_bytes());
        chunk.extend(payload);
        if !chunk.len().is_multiple_of(2) {
            chunk.push(0);
        }
        chunk
    }

    let vp8x = [
        b'V', b'P', b'8', b'X', 10, 0, 0, 0, 0x02, 0, 0, 0, 99, 0, 0, 79, 0, 0,
    ];
    let anim = [b'A', b'N', b'I', b'M', 6, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    let mut body = Vec::new();
    body.extend_from_slice(&vp8x);
    body.extend_from_slice(&anim);
    body.extend(anmf_chunk(100));
    body.extend(anmf_chunk(200));

    let riff_size = u32::try_from(body.len() + 4).expect("webp body fits u32");
    let mut out = vec![b'R', b'I', b'F', b'F'];
    out.extend_from_slice(&riff_size.to_le_bytes());
    out.extend_from_slice(b"WEBP");
    out.extend(body);
    out
}
