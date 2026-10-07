// Synthetic static AVIF ftyp stub for CI runtime probe regression (no external fixture).

/// Minimal ISOBMFF `ftyp` with major brand `avif` (12-byte box), padded for header readers.
#[must_use]
fn build_synthetic_static_avif_ftyp() -> Vec<u8> {
    let mut bytes = vec![
        0, 0, 0, 20, b'f', b't', b'y', b'p', b'a', b'v', b'i', b'f', 0, 0, 0, 0, b'a', b'v',
        b'i', b'f',
    ];
    bytes.resize(64, 0);
    bytes
}
