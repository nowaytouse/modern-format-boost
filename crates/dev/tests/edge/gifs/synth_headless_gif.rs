// Synthetic headless GIF: multiple image descriptors without Graphics Control delays.

/// Build a 7-frame `GIF87a` sticker with no `GCE`/delay blocks (headless regression asset).
#[must_use]
fn build_synthetic_headless_sticker_gif() -> Vec<u8> {
    fn image_descriptor_frame() -> Vec<u8> {
        let mut out = vec![
            0x2C, 0x00, 0x00, 0x00, 0x00, // image descriptor, position 0,0
            0x01, 0x00, 0x01, 0x00, // 1x1
            0x00, // packed: no local color table
        ];
        // Minimal LZW image data sub-blocks (2-byte payload + terminator)
        out.extend_from_slice(&[0x02, 0x02, 0x4C, 0x01, 0x00]);
        out
    }

    let mut gif = Vec::new();
    gif.extend_from_slice(b"GIF87a");
    // Logical screen descriptor: 10x10, no global color table
    gif.extend_from_slice(&[0x0A, 0x00, 0x0A, 0x00, 0x00, 0x00]);
    gif.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);

    for _ in 0..7 {
        gif.extend_from_slice(&image_descriptor_frame());
    }
    gif.push(0x3B);
    gif
}
