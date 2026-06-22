// Synthetic static HEIC/HEIF ftyp stubs for CI runtime probe regression (no external fixture).

// Minimal ISOBMFF `ftyp` with major brand `heic` (12-byte box), padded for header readers.
#[must_use]
fn build_synthetic_static_heic_ftyp() -> Vec<u8> {
    let mut bytes = vec![0, 0, 0, 12, b'f', b't', b'y', b'p', b'h', b'e', b'i', b'c'];
    bytes.resize(64, 0);
    bytes
}

// `ftyp` + minimal `moov`/`mvhd` (no tracks) — ffprobe may parse further than header-only stub.
#[must_use]
fn build_synthetic_static_heic_ftyp_moov() -> Vec<u8> {
    let mut bytes = build_synthetic_static_heic_ftyp();
    append_minimal_moov_mvhd(&mut bytes);
    bytes
}

const SYNTH_MVHD_BOX_LEN: u32 = 108;
const SYNTH_MOOV_BOX_LEN: u32 = 8 + SYNTH_MVHD_BOX_LEN;

fn append_minimal_moov_mvhd(out: &mut Vec<u8>) {
    let mut mvhd = [0u8; 100];
    mvhd[12..16].copy_from_slice(&1u32.to_be_bytes());
    mvhd[20..24].copy_from_slice(&0x0001_0000u32.to_be_bytes());
    mvhd[24..26].copy_from_slice(&0x0100u16.to_be_bytes());
    mvhd[96..100].copy_from_slice(&2u32.to_be_bytes());
    out.extend_from_slice(&SYNTH_MOOV_BOX_LEN.to_be_bytes());
    out.extend_from_slice(b"moov");
    out.extend_from_slice(&SYNTH_MVHD_BOX_LEN.to_be_bytes());
    out.extend_from_slice(b"mvhd");
    out.extend_from_slice(&mvhd);
}

// `mif1` major brand with `heic` compatible brand (disambiguation path).
#[must_use]
fn build_synthetic_mif1_heic_compat_ftyp() -> Vec<u8> {
    let mut bytes = vec![
        0, 0, 0, 20, b'f', b't', b'y', b'p', b'm', b'i', b'f', b'1', 0, 0, 0, 0, b'h', b'e',
        b'i', b'c',
    ];
    bytes.resize(64, 0);
    bytes
}

// AVIF image sequence: major brand `avis` (ISOBMFF animated sequence).
#[must_use]
fn build_synthetic_animated_avif_avis_ftyp() -> Vec<u8> {
    let mut bytes = vec![
        0, 0, 0, 20, b'f', b't', b'y', b'p', b'a', b'v', b'i', b's', 0, 0, 0, 0, b'a',
        b'v', b'i', b'f',
    ];
    bytes.resize(64, 0);
    bytes
}

// Animated HEIF: major brand `msf1` (multi-sample) with `heic` compatible brand.
#[must_use]
fn build_synthetic_animated_heif_msf1_ftyp() -> Vec<u8> {
    let mut bytes = vec![
        0, 0, 0, 20, b'f', b't', b'y', b'p', b'm', b's', b'f', b'1', 0, 0, 0, 0, b'h', b'e',
        b'i', b'c',
    ];
    bytes.resize(64, 0);
    bytes
}
