//! AVIF / AV1 (MP4) output health checks.
//!
//! Minimal post-conversion validation. Stricter checks (e.g. ffprobe probe or
//! minimum length / keyframe checks) can be added later if needed.

use std::path::Path;

const MIN_FILE_LEN: u64 = 32;

/// Verify AVIF output exists and has minimal size. Optional ffprobe or
/// signature checks can be added later.
pub fn verify_avif_health(path: &Path) -> Result<(), String> {
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("Not a file".to_string());
    }
    if meta.len() < MIN_FILE_LEN {
        return Err(format!("AVIF file too small ({} bytes)", meta.len()));
    }
    Ok(())
}

/// Verify AV1-in-MP4 output exists and has minimal size. Optional ffprobe
/// or duration checks can be added later.
pub fn verify_av1_mp4_health(path: &Path) -> Result<(), String> {
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("Not a file".to_string());
    }
    if meta.len() < MIN_FILE_LEN {
        return Err(format!("AV1 MP4 file too small ({} bytes)", meta.len()));
    }
    Ok(())
}
