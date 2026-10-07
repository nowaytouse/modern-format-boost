// Synthetic animated PNG for CI runtime probe regression (no external fixture).

/// Two-frame APNG (1×1) for header preflight / `detect_video` regression.
#[must_use]
fn build_synthetic_two_frame_apng() -> Vec<u8> {
    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = u32::MAX;
        for &byte in bytes {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = (crc >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(crc & 1));
            }
        }
        !crc
    }

    fn png_chunk(chunk_type: &[u8], payload: &[u8]) -> Vec<u8> {
        let mut chunk = Vec::with_capacity(12 + payload.len());
        chunk.extend_from_slice(
            &u32::try_from(payload.len())
                .expect("apng chunk payload fits u32")
                .to_be_bytes(),
        );
        chunk.extend_from_slice(chunk_type);
        chunk.extend_from_slice(payload);
        let mut crc_input = chunk_type.to_vec();
        crc_input.extend_from_slice(payload);
        chunk.extend_from_slice(&crc32(&crc_input).to_be_bytes());
        chunk
    }

    fn fctl_chunk(sequence: u32, delay_num: u16, delay_den: u16) -> Vec<u8> {
        let mut payload = vec![0u8; 26];
        payload[0..4].copy_from_slice(&sequence.to_be_bytes());
        payload[7] = 1;
        payload[11] = 1;
        payload[20] = foundation::numeric_cast::u16_high8_to_u8(delay_num);
        payload[21] = foundation::numeric_cast::u16_low8_to_u8(delay_num);
        payload[22] = foundation::numeric_cast::u16_high8_to_u8(delay_den);
        payload[23] = foundation::numeric_cast::u16_low8_to_u8(delay_den);
        png_chunk(b"fcTL", &payload)
    }

    let mut data = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let ihdr = [0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0];
    data.extend(png_chunk(b"IHDR", &ihdr));
    data.extend(png_chunk(b"acTL", &[0, 0, 0, 2, 0, 0, 0, 0]));
    data.extend(fctl_chunk(0, 1, 100));
    data.extend(png_chunk(
        b"IDAT",
        &[0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01],
    ));
    data.extend(fctl_chunk(1, 2, 100));
    let mut second_frame = 2u32.to_be_bytes().to_vec();
    second_frame.extend_from_slice(&[
        0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01,
    ]);
    data.extend(png_chunk(b"fdAT", &second_frame));
    data.extend(png_chunk(b"IEND", &[]));
    data
}
