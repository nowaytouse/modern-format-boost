// Synthetic static JXL header stubs for CI runtime probe regression (no external fixture).

use foundation::constants::{JXL_HEADER_LONG, JXL_HEADER_SHORT};

/// Short JXL codestream prefix (`FF 0A`).
#[must_use]
fn build_synthetic_jxl_short_header() -> Vec<u8> {
    let mut header = vec![0u8; 64];
    header[..JXL_HEADER_SHORT.len()].copy_from_slice(JXL_HEADER_SHORT);
    header
}

/// Container-style JXL signature (`....JXL `).
#[must_use]
fn build_synthetic_jxl_long_header() -> Vec<u8> {
    let mut header = vec![0u8; 64];
    header[..JXL_HEADER_LONG.len()].copy_from_slice(JXL_HEADER_LONG);
    header
}
