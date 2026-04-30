pub fn build_synthetic_animated_webp_without_vp8x_in_header() -> Vec<u8> {
    // Minimal RIFF WEBP container with ANIM + ANMF markers placed beyond the first 64 bytes.
    // This does not need to be a decodable image; it exists to lock animation classification logic.
    let mut bytes = Vec::new();

    // RIFF header (size placeholder; not used by our detectors)
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(b"WEBP");

    // Pad so VP8X is not in the first 64 bytes (and we don't include it at all)
    bytes.extend(std::iter::repeat_n(0u8, 80));

    // Insert animation markers
    bytes.extend_from_slice(b"ANIM");
    bytes.extend(std::iter::repeat_n(0u8, 16));

    // Two frames markers
    bytes.extend_from_slice(b"ANMF");
    bytes.extend(std::iter::repeat_n(0u8, 16));
    bytes.extend_from_slice(b"ANMF");
    bytes.extend(std::iter::repeat_n(0u8, 16));

    bytes
}

